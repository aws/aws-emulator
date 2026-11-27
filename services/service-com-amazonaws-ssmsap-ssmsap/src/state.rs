// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmsap-ssmsap service.
//!
//! The registered SAP application is this service's primary resource: every
//! application-scoped operation (`RegisterApplication`, `GetApplication`,
//! `ListApplications`, `StartApplication`, `StopApplication`,
//! `StartApplicationRefresh`, `UpdateApplicationSettings`, `DeregisterApplication`)
//! resolves the SAME
//! [`StoredApplication`] out of
//! [`RegionState::applications`], and every call that changes one files a
//! [`StoredOperation`] in [`RegionState::operations`] that `GetOperation` reads back and
//! `ListOperations` reports per application ([`RegionState::operations_for`]).
//! So a register is observable by every later read, a deregister by every later call,
//! and an `OperationId` handed to a caller always names a record that exists.
//!
//! Those calls are ASYNCHRONOUS, so what they start is a change the CLOCK owes the
//! application ([`PendingTransition`]): the next status it takes, or the settings an
//! update asked for ([`PendingSettings`]). Until that change falls due the
//! application resolves in the transient status the call gave it (`REGISTERING`,
//! `STARTING`, `STOPPING`), and [`RegionState::settle`] materialises the change for
//! every reader at the same instant without anything ever sleeping.
//!
//! A DEREGISTRATION is NOT one of them: it ends the registration at the instant the call
//! is accepted, so the application — and everything held for it — stops resolving before
//! the response is written ([`RegionState::deregister_application`]) and every later call
//! naming that id answers the condition that the account has no such registered
//! application ([`no_registered_application`], [`application_not_found_message`]).
//!
//! WHICH change a registration owes is the outcome of the DISCOVERY it runs on the Amazon
//! EC2 instances it named ([`discovery::run_document`]): a discovery that can be carried
//! out brings the application to `ACTIVATED`/`SUCCESS`, and one that cannot brings it to
//! `FAILED`/`REGISTRATION_FAILED` with the refusal it ran into on the operation record
//! the registration filed ([`StoredOperation::terminal_status`]). So a registration is
//! reported as successful only where the instances it named really can be reached, and
//! every call whose precondition is a working application reads that outcome off the
//! application ([`StoredApplication::registered_successfully`],
//! [`StoredApplication::registration_failed`]) rather than off what else is in flight.
//!
//! Those records are held PER REGION: the service is regional, so the store is a
//! [`RegionState`] per Region ([`ComAmazonawsSsmsapSsmsapState::region`]) and a handler
//! only ever reaches the partition of the Region its request was signed for
//! ([`request_region`]).
//!
//! A configuration check is the service's OTHER asynchronous unit of work:
//! `StartConfigurationChecks` files one [`StoredConfigurationCheckOperation`] per
//! requested check in [`RegionState::configuration_check_operations`], which
//! `GetConfigurationCheckOperation` reads back by its id and
//! `ListConfigurationCheckOperations` reports per application
//! ([`RegionState::configuration_check_operations_for`]) — in that Region alone, like
//! every other record.
//!
//! WHICH checks there are to run is the service's own catalogue rather than an account's
//! record: [`configuration_check_definitions`] is the set `ListConfigurationCheckDefinitions`
//! reports and the set [`configuration_checks_to_run`] starts when a request names no
//! check, so the two cannot disagree about what this service supports. Both read the
//! check's prose from the one [`CONFIGURATION_CHECK_CATALOGUE`] every
//! `ConfigurationCheckOperation` names its check with.
//!
//! The record is the stored half; the wire half (`Application`, `ApplicationSummary`,
//! `Operation`, `ConfigurationCheckOperation`, `ComponentSummary`) is PROJECTED from it
//! by [`StoredApplication::view`] / [`StoredApplication::summary`] /
//! [`StoredOperation::view`] / [`StoredConfigurationCheckOperation::view`] /
//! [`StoredComponent::summary`], so no handler assembles a response member by hand.
//!
//! An application's SAP HANA databases are DERIVED from that same record rather than
//! held beside it: the emulator discovers nothing on the registered instances, so the
//! databases an application has are the ones its registration named — by `DatabaseArn`
//! or by a credential's `DatabaseName`
//! ([`StoredApplication::databases`], projected onto the wire `DatabaseSummary` by
//! [`ApplicationDatabase::summary`]) — which is what `ListDatabases` reports, so a
//! database cannot outlive the registration it came from. `GetDatabase` reads ONE of
//! those same records, resolved by its ARN or its `DatabaseId`
//! ([`RegionState::resolve_database`]) and projected onto the wire `Database` by
//! [`StoredApplication::database_view`], so the database a caller reads one by one is the
//! database the listing reports.
//!
//! Those databases are also what ASSOCIATES two registered applications with one
//! another: an application whose SAP HANA connection names the database of another
//! registered application shares that database with it, so each is an associated
//! application of the other. The relation lives between the two records rather than on
//! either, so it is derived from the same database sets
//! ([`RegionState::associated_application_arns`]) at the instant an `Application` is
//! projected ([`RegionState::application_view`]) — which is what
//! `Application$AssociatedApplicationArns` reports, whichever call last set that
//! connection (`RegisterApplication$DatabaseArn`,
//! `UpdateApplicationSettings$DatabaseArn`).
//!
//! The database management system those databases live in is the one thing held BESIDE
//! that record ([`StoredApplication::dbms`]), because the application and its connected
//! DBMS are started and stopped separately: `StopApplication` stops the DBMS only when
//! it names it as the connected entity to stop, so `Database$Status` reports the DBMS's
//! own status ([`StoredDbms::status_for`]) rather than a second reading of the
//! application's.
//!
//! An application's COMPONENTS are held the same way, ON the application rather than in
//! a map of their own (a `ComponentId` names a component only within its application):
//! the [`StoredComponent`] records [`StoredApplication::components`] answers are the ones
//! the service's DISCOVERY of that application produced, so an application whose
//! discovery has not completed has none at all. What a registration describes
//! ([`registered_components`]) — each `ComponentsInfo` entry, plus the SAP HANA component
//! of the SAP system the registration's own `Sid` names, which is the component its
//! databases belong to — is the landscape that discovery finds
//! ([`StoredApplication::described_components`]), not an inventory of components. Each
//! discovered component is projected onto the wire `Component` by
//! [`StoredApplication::component_view`] — which is what `GetComponent` reads back — and
//! onto `ComponentSummary` by [`StoredComponent::summary`], which is what
//! `ListComponents` reports. So a component a caller can read is one discovery produced,
//! every `ComponentId` another response reports is one of them, it reports the status its
//! application is in, and the databases it reports are the ones that registration gave
//! it.
//!
//! What a configuration check RUN produced hangs off that same record: the sub-check
//! results in [`StoredConfigurationCheckOperation::sub_check_results`], each holding the
//! rule results of its own sub-check ([`StoredSubCheckResult::rule_results`]), which is
//! what `ListSubCheckResults` and `ListSubCheckRuleResults` page over. So a result a
//! caller can read is one some run produced, and it is forgotten exactly when the run
//! that produced it is.
//!
//! WHAT a run produces is the EVALUATION of its check's rules against the registered
//! application: [`CONFIGURATION_CHECK_CATALOGUE`] is the sub-checks and
//! [`ConfigurationRule`]s each `ConfigurationCheckType` runs, and each rule reads its
//! status off that application's own record, so the same check passes for one
//! registration and fails for another. The run reaches `SUCCESS` with those results when
//! the clock brings it there, and the `RuleStatusCounts` its record reports are the tally
//! of them ([`StoredConfigurationCheckOperation::rule_status_counts`]) rather than a
//! second copy.
//!
//! What an operation PASSED is derived from that same operation record rather than held
//! beside it: its milestones ([`StoredOperation::events`], projected onto the wire
//! `OperationEvent` by [`OperationEventRecord::view`]) are the start of its work and, once
//! the clock has brought the record to a terminal status, the instant it reached it — so
//! `ListOperationEvents` cannot report a milestone of an operation nobody filed, and the
//! milestones a caller reads move exactly when `GetOperation`'s status does.
//!
//! Resource permissions work the same way: `PutResourcePermission` grants,
//! `DeleteResourcePermission` revokes and `GetResourcePermission` reads the SAME triples in
//! [`RegionState::resource_permissions`],
//! and the `Policy` document all three report is PROJECTED from them by
//! [`RegionState::resource_policy`] — so a grant is observable by every later read in
//! that Region, a revocation disappears from every later read at once, and no handler
//! writes a policy document of its own. The target of a
//! grant is a resource this Region holds ([`RegionState::holds_resource`], the same
//! resolution the tagging calls run), so the policy a read reports always belongs to a
//! resource some registration brought into being, and the grants go when it does.

/// The hop into Amazon EC2: the one client this crate asks the owner of an instance
/// through.
pub mod ec2;

/// The discovery a registration runs on the Amazon EC2 instances it named, which is what
/// decides whether that registration can complete.
pub mod discovery;

/// Shutting down the Amazon EC2 instances of an application through the service that
/// owns them, which is what `StopApplication$IncludeEc2InstanceShutdown` asks for.
pub mod ec2_shutdown;
/// This service's configuration check catalogue: the rules a check evaluates, the entry
/// each check is published under, and the records a run of one leaves behind. Re-exported
/// here, so every caller reaches them as `crate::state::<item>` exactly as before.
mod configuration_checks;
pub use configuration_checks::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Application, ApplicationCredential, ApplicationSummary, AssociatedHost, BackintConfig,
    Component, ComponentInfo, ComponentSummary, ConfigurationCheckDefinition,
    ConfigurationCheckOperation, ConflictException, Database, DatabaseSummary, Filter,
    InternalServerException, Operation, OperationEvent, Resource, ResourceNotFoundException,
    RuleResult, RuleStatusCounts, SubCheckResult, ValidationException,
    APPLICATION_TYPE_VALUES, CONFIGURATION_CHECK_TYPE_VALUES, FILTER_OPERATOR_VALUES,
    RULE_RESULT_STATUS_VALUES,
};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own generated config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, as the generated
/// `OPERATIONS.md` reports it — the `service` segment of every application ARN.
pub const ARN_SERVICE: &str = "ssm-sap";

/// The resource-type segment of the ARN of a registered application
/// ([`application_arn`]).
///
/// The first segment of an ARN's resource field names the KIND of resource the ARN is
/// of, so these three constants are both what the ARNs this service mints are built from
/// and what [`ssm_sap_resource_type`] reads back off one.
pub const ARN_RESOURCE_TYPE_APPLICATION: &str = "application";

/// The resource-type segment of the ARN of a component of a registration
/// ([`component_arn`]). See [`ARN_RESOURCE_TYPE_APPLICATION`].
pub const ARN_RESOURCE_TYPE_COMPONENT: &str = "component";

/// The resource-type segment of the ARN of a SAP HANA database of a registration
/// ([`database_arn`]). See [`ARN_RESOURCE_TYPE_APPLICATION`].
pub const ARN_RESOURCE_TYPE_DATABASE: &str = "database";

/// `ApplicationStatus` while registration is still settling. One of the model's
/// `ApplicationStatus` values (see [`crate::ops::types::APPLICATION_STATUS_VALUES`]).
pub const APPLICATION_STATUS_REGISTERING: &str = "REGISTERING";

/// `ApplicationStatus` for an application that is registered and running.
pub const APPLICATION_STATUS_ACTIVATED: &str = "ACTIVATED";

/// `ApplicationStatus` between `StartApplication` and the application running.
pub const APPLICATION_STATUS_STARTING: &str = "STARTING";

/// `ApplicationStatus` between `StopApplication` and the application being down.
pub const APPLICATION_STATUS_STOPPING: &str = "STOPPING";

/// `ApplicationStatus` for an application that has been stopped.
pub const APPLICATION_STATUS_STOPPED: &str = "STOPPED";

/// `ApplicationStatus` of an application whose registration did not complete: the
/// registration ran and its discovery could not be carried out, so the service manages
/// nothing for it ([`StoredApplication::registration_failed`]).
pub const APPLICATION_STATUS_FAILED: &str = "FAILED";

/// `ApplicationDiscoveryStatus` while the registration's discovery is still running.
/// One of the model's `ApplicationDiscoveryStatus` values (see
/// [`crate::ops::types::APPLICATION_DISCOVERY_STATUS_VALUES`]).
pub const DISCOVERY_STATUS_REGISTERING: &str = "REGISTERING";

/// `ApplicationDiscoveryStatus` once discovery has completed.
pub const DISCOVERY_STATUS_SUCCESS: &str = "SUCCESS";

/// `ApplicationDiscoveryStatus` of an application whose registration's discovery could
/// not be carried out: the discovery result a failed registration converges to.
pub const DISCOVERY_STATUS_REGISTRATION_FAILED: &str = "REGISTRATION_FAILED";

/// `ApplicationDiscoveryStatus` of an application whose RE-discovery could not be carried
/// out: the discovery result a failed refresh converges to.
///
/// The model spells a failed discovery two ways — one for the discovery a registration
/// runs ([`DISCOVERY_STATUS_REGISTRATION_FAILED`]) and one for the discovery a refresh
/// re-runs — so what `Application$DiscoveryStatus` reports as "the latest discovery
/// result" says WHICH discovery produced it. A refresh that could not be carried out
/// leaves the registration itself intact: the application keeps the components that
/// registration discovered and its own `Status`, and only its latest discovery result
/// failed, so a later refresh of the same application can report `SUCCESS` again.
pub const DISCOVERY_STATUS_REFRESH_FAILED: &str = "REFRESH_FAILED";

/// `OperationStatus` for an operation that has not reached its terminal status yet.
/// One of the model's `OperationStatus` values (see
/// [`crate::ops::types::OPERATION_STATUS_VALUES`]).
pub const OPERATION_STATUS_INPROGRESS: &str = "INPROGRESS";

/// `OperationStatus` for an operation that completed.
pub const OPERATION_STATUS_SUCCESS: &str = "SUCCESS";

/// `OperationStatus` for an operation that ended without doing its work. One of the
/// model's `OperationStatus` values (see [`crate::ops::types::OPERATION_STATUS_VALUES`]).
pub const OPERATION_STATUS_ERROR: &str = "ERROR";

/// `Operation$Type` for the operation `RegisterApplication` files.
///
/// `OperationType` is an unconstrained string in the model (no enum), so the value is
/// the name of the API call that produced the record — which is what makes a record
/// readable next to the call a client made.
pub const OPERATION_TYPE_REGISTER_APPLICATION: &str = "REGISTER_APPLICATION";

/// `Operation$Type` for the operation `StartApplication` files.
pub const OPERATION_TYPE_START_APPLICATION: &str = "START_APPLICATION";

/// `Operation$Type` for the operation `StopApplication` files.
pub const OPERATION_TYPE_STOP_APPLICATION: &str = "STOP_APPLICATION";

/// `Operation$Type` for the operation `DeregisterApplication` files.
pub const OPERATION_TYPE_DEREGISTER_APPLICATION: &str = "DEREGISTER_APPLICATION";

/// `Operation$Type` for the operation `StartApplicationRefresh` files, spelled by the
/// same rule as the records above: the name of the API call that produced it.
pub const OPERATION_TYPE_START_APPLICATION_REFRESH: &str = "START_APPLICATION_REFRESH";

/// `Operation$Type` for the operation `UpdateApplicationSettings` files, spelled by the
/// same rule as the records above: the name of the API call that produced it. ONE record
/// per call, whatever the call changes, because the unit of work the service reports is
/// the call it was asked for — which is the id `UpdateApplicationSettings$OperationIds`
/// reports and `GetOperation` reads back.
pub const OPERATION_TYPE_UPDATE_APPLICATION_SETTINGS: &str = "UPDATE_APPLICATION_SETTINGS";

/// `Operation$ResourceType`: the kind of resource an operation acted on.
///
/// `ResourceType` is an unconstrained string of length 1..64 in the model, so this
/// names the resource kind every operation in this slice targets — the registered
/// application.
pub const APPLICATION_RESOURCE_TYPE: &str = "APPLICATION";

/// `Resource$ResourceType` for the registered application an operation acted on.
///
/// `Operation$ResourceType` is an unconstrained string ([`APPLICATION_RESOURCE_TYPE`]),
/// while the `Resource` an `OperationEvent` carries holds an `OperationEventResourceType`
/// — pattern `^[\w]+::[\w]+::[\w]+$`, which `Resource$ResourceType`'s documentation
/// spells `AWS::SystemsManagerSAP::Component` for this service's component resource — so
/// this is that same spelling for its application resource. See [`event_resource_type`].
pub const OPERATION_EVENT_RESOURCE_TYPE_APPLICATION: &str = "AWS::SystemsManagerSAP::Application";

/// `Resource$ResourceType` of an Amazon EC2 instance an operation acted on.
///
/// The spelling `Resource$ResourceType`'s own documentation gives for that resource
/// (`smithy_query(query="doc", shape="com.amazonaws.ssmsap#Resource",
/// member="ResourceType")` lists it beside the component one), so a milestone about an
/// instance names its kind exactly as the model does.
pub const OPERATION_EVENT_RESOURCE_TYPE_EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// `OperationEventStatus` of a milestone of an operation that is still running. One of
/// the model's `OperationEventStatus` values (see
/// [`crate::ops::types::OPERATION_EVENT_STATUS_VALUES`]).
pub const OPERATION_EVENT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `OperationEventStatus` of the milestone an operation that finished its work reached.
pub const OPERATION_EVENT_STATUS_COMPLETED: &str = "COMPLETED";

/// `OperationEventStatus` of the milestone an operation that ended without doing its work
/// reached.
pub const OPERATION_EVENT_STATUS_FAILED: &str = "FAILED";

/// `ComponentType` of the SAP HANA database component of an application. One of the
/// model's `ComponentType` values (see [`crate::ops::types::COMPONENT_TYPE_VALUES`]).
pub const COMPONENT_TYPE_HANA: &str = "HANA";

/// `ComponentStatus` of a component of an application that is up. One of the model's
/// `ComponentStatus` values (see [`crate::ops::types::COMPONENT_STATUS_VALUES`]).
pub const COMPONENT_STATUS_RUNNING: &str = "RUNNING";

/// `ComponentStatus` of a component being started.
pub const COMPONENT_STATUS_STARTING: &str = "STARTING";

/// `ComponentStatus` of a component being stopped.
pub const COMPONENT_STATUS_STOPPING: &str = "STOPPING";

/// `ComponentStatus` of a component that is not running.
pub const COMPONENT_STATUS_STOPPED: &str = "STOPPED";

/// `ComponentStatus` for a component whose status the service cannot state from what it
/// knows about the application — which is what the model documents this value for.
pub const COMPONENT_STATUS_UNDEFINED: &str = "UNDEFINED";

/// The name SAP HANA gives the system database of a multitenant database system; every
/// other database of that system is a tenant database. It is what tells a `DatabaseType`
/// of `SYSTEM` from one of `TENANT` — see [`database_type_for`].
pub const HANA_SYSTEM_DATABASE_NAME: &str = "SYSTEMDB";

/// `DatabaseType` of the system database of a HANA system. One of the model's
/// `DatabaseType` values (see [`crate::ops::types::DATABASE_TYPE_VALUES`]).
pub const DATABASE_TYPE_SYSTEM: &str = "SYSTEM";

/// `DatabaseType` of a tenant database of a HANA system.
pub const DATABASE_TYPE_TENANT: &str = "TENANT";

/// `DatabaseStatus` of a database of an application that is up. One of the model's
/// `DatabaseStatus` values (see [`crate::ops::types::DATABASE_STATUS_VALUES`]).
pub const DATABASE_STATUS_RUNNING: &str = "RUNNING";

/// `DatabaseStatus` of a database being started.
pub const DATABASE_STATUS_STARTING: &str = "STARTING";

/// `DatabaseStatus` of a database being stopped.
pub const DATABASE_STATUS_STOPPING: &str = "STOPPING";

/// `DatabaseStatus` of a database that is not running.
pub const DATABASE_STATUS_STOPPED: &str = "STOPPED";

/// `DatabaseStatus` for a database whose status the service cannot state from what it
/// knows about the application — which is what the model documents this value for.
pub const DATABASE_STATUS_UNKNOWN: &str = "UNKNOWN";

/// The longest `DatabaseId` the model admits
/// (`smithy_query(query="shape", shape="DatabaseSummary")`: `DatabaseId` length 1..300),
/// which is what [`database_id_from_name`] cuts a derived id to.
pub const MAX_DATABASE_ID_LENGTH: usize = 300;

/// How long after the call that starts it a status change the clock owes falls due.
///
/// Registering, starting and stopping an SAP application are asynchronous in the real
/// service: the call returns an `OperationId` and the application reaches its next
/// status later. Nothing AWS documents fixes how long that takes, so this is the
/// emulator's own span — long enough that a caller acting straight after the call still
/// observes the transient `REGISTERING`/`STARTING`/`STOPPING` status and the
/// `INPROGRESS` operation, short enough that a test need not wait long. The due instant
/// is STORED on the record and materialised by the first request at or after it
/// ([`StoredApplication::settle`] / [`StoredOperation::settle`]); nothing ever sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// How many applications ONE account may have registered in ONE Region: the account's
/// Applications quota for AWS Systems Manager for SAP.
///
/// A registration is scoped to an account and a Region ([`RegionState`]), and the service
/// allots an account a fixed number of registered applications there; the registration
/// that would exceed the allotment is refused and nothing is written for it
/// ([`applications_quota_message`], the modelled `ValidationException`). The allotment is
/// the quota this service is recorded answering with rather than a number this crate
/// chose, and it is checked against the applications the Region currently holds, so
/// deregistering one frees a slot and the identical call then succeeds. It is also what
/// bounds how much one Region's partition can hold.
pub const APPLICATIONS_QUOTA: usize = 10;

/// How many Region partitions this emulator will hold.
///
/// The store is partitioned per Region (see [`RegionState`]) and the Region comes from
/// the caller's own SigV4 credential scope, so without a ceiling a caller could mint a
/// partition per made-up Region. Comfortably more than the Regions AWS operates, so no
/// real multi-Region workflow meets it; with [`APPLICATIONS_QUOTA`],
/// [`MAX_RETAINED_OPERATIONS`] and the per-resource ceilings
/// ([`MAX_CREDENTIALS_PER_APPLICATION`], [`MAX_TAGS_PER_RESOURCE`],
/// [`MAX_PERMISSIONED_RESOURCES`]) it is what bounds the whole store. Like the
/// application
/// ceiling it is reported with `ConflictException`, which `RegisterApplication` models,
/// and a partition left empty is dropped
/// ([`ComAmazonawsSsmsapSsmsapState::settle`]) so only Regions that
/// hold something count against it.
pub const MAX_REGIONS: usize = 64;

/// How many operation records are retained per Region, per kind of record (application
/// operations, configuration check operations). Beyond this the oldest record that has
/// already reached a terminal status is dropped first, so a caller polling a fresh
/// operation can still resolve it.
pub const MAX_RETAINED_OPERATIONS: usize = 10_000;

/// The page size every listing of this service takes when the caller names no
/// `MaxResults`, and the ceiling each clamps a requested page to: every one of them
/// declares the same `MaxResults` range of 1..50, so 50 is both the largest page the API
/// allows and the default page.
pub const MAX_PAGE_SIZE: i32 = 50;

/// How many components ONE registration may DECLARE in its `ComponentsInfo`.
///
/// The model gives `ComponentInfoList` `@length(min: 0, max: 5)`
/// (`smithy_query(query="shape", shape="ComponentInfoList")`), so the boundary validator
/// already refuses a longer `ComponentsInfo`; [`components_within_bounds`] re-states the
/// bound HERE so the work every component-scoped call does is bounded by a check in this
/// crate rather than by one that lives elsewhere. It is reported with
/// `ValidationException`, which `RegisterApplication` models.
pub const MAX_DECLARED_COMPONENTS: usize = 5;

/// How many components ONE registered application holds: the ones its `ComponentsInfo`
/// declared, plus the one SAP HANA component the registration's own `Sid` identifies
/// ([`registered_components`]) when no declaration already describes it.
pub const MAX_COMPONENTS_PER_APPLICATION: usize = MAX_DECLARED_COMPONENTS + 1;

/// How many components one Region can hold, derived from the two ceilings that bound
/// it: at most [`APPLICATIONS_QUOTA`] applications, each with at most
/// [`MAX_COMPONENTS_PER_APPLICATION`] components. So the set a component listing walks
/// is bounded by a constant, independent of how many requests a caller makes. Both
/// ceilings are enforced where a component is written, so this is the invariant that
/// follows from them rather than a third check: a listing states it as a debug-build
/// assertion and still reports every component it holds.
pub const MAX_COMPONENTS_PER_REGION: usize = APPLICATIONS_QUOTA * MAX_COMPONENTS_PER_APPLICATION;

/// Refuse a registration that declares more components than one `ComponentsInfo` may
/// carry ([`MAX_DECLARED_COMPONENTS`]).
///
/// `Ok(())` when the declaration is within the bound. The boundary validator enforces
/// the same bound from the model, so this cannot normally fire; it is here so the size
/// of what one registration stores — and of what a later listing walks — is bounded by
/// a check in this crate. The caller must apply it BEFORE writing anything, so a refused
/// registration leaves no application behind.
pub fn components_within_bounds(
    components_info: &[ComponentInfo],
) -> Result<(), ValidationException> {
    if components_info.len() > MAX_DECLARED_COMPONENTS {
        return Err(validation(format!(
            "ComponentsInfo holds at most {MAX_DECLARED_COMPONENTS} components, but {} were given",
            components_info.len()
        )));
    }
    Ok(())
}

/// The `Version` of a rendered resource policy document: the current IAM policy
/// language version, which is what every AWS resource policy document carries.
pub const POLICY_VERSION: &str = "2012-10-17";

/// The `Effect` of a statement a resource permission grant renders to. A grant adds
/// permission, so the statement it renders to allows.
pub const POLICY_EFFECT_ALLOW: &str = "Allow";

/// How many distinct target resources one Region will hold permissions for, and how
/// many source resources may be granted one action type on one target.
///
/// The real service caps a resource policy by document size rather than by grant
/// count, so neither number is a fidelity claim about a published quota: together they
/// are the emulator's own ceiling, generous enough that no realistic workflow meets
/// them, so a caller looping on `PutResourcePermission` cannot grow this process's
/// memory without bound. Both are checked in constant time and reported with
/// `ValidationException`, which `PutResourcePermission` models.
pub const MAX_PERMISSIONED_RESOURCES: usize = 1_000;

/// How many source resources may hold one action type on one target. See
/// [`MAX_PERMISSIONED_RESOURCES`].
pub const MAX_PERMISSION_SOURCES_PER_RESOURCE: usize = 100;

/// How many tags this emulator will hold for ONE resource.
///
/// Neither the model nor this operation's documentation states a tag count, so this is
/// the emulator's own ceiling rather than a fidelity claim about a published quota: it
/// is what stops a caller looping on `TagResource` with fresh keys growing this
/// process's memory without bound. It is reported with `ValidationException`, which
/// every tagging call models.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The expression the model declares for a tag key
/// (`smithy_query(query="shape", shape="TagKey")`: `^(?!aws:)[a-zA-Z+-=._:/]+$`), which
/// is the key shape of the one `TagMap` every tagging path of this service carries.
///
/// Re-stated here because NOTHING ahead of a handler applies it. The validating walk
/// treats a map's KEY as a path segment rather than as a constrained leaf, so it applies
/// no constraint a key shape declares at all; and this expression opens with a negative
/// lookahead, which the walk's regex engine does not compile, so even where a tag key
/// IS a constrained leaf (the `TagKeyList` `UntagResource` binds to its query string)
/// every value satisfies it. So the rule is the handler's to apply, and
/// [`tag_key_within_modelled_form`] is the one reading of it.
pub const TAG_KEY_PATTERN: &str = r"^(?!aws:)[a-zA-Z+-=._:/]+$";

/// The prefix [`TAG_KEY_PATTERN`]'s lookahead excludes: AWS reserves `aws:`-prefixed tag
/// keys for the tags it sets itself, so no caller may attach, change or remove one.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The member path a clause names a `TagMap` at: both calls that carry one bind it to a
/// `Tags` member, which a message states in the lowerCamel the validating walk spells a
/// member in.
pub const TAGS_MEMBER_PATH: &str = "tags";

/// The member path a clause names `UntagResource`'s key list at, spelled as the model
/// spells that member (`@httpQuery("tagKeys")`).
pub const TAG_KEYS_MEMBER_PATH: &str = "tagKeys";

/// Whether `key` is a tag key [`TAG_KEY_PATTERN`] admits.
///
/// The expression read as it is written: at least one character, every one of them inside
/// its character class — the ASCII letters, `_`, and the `+`-to-`=` range its `+-=`
/// spells, which covers `+ , - . / 0-9 : ; < =` — and not the reserved prefix its
/// lookahead excludes ([`RESERVED_TAG_KEY_PREFIX`]).
pub fn tag_key_within_modelled_form(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(RESERVED_TAG_KEY_PREFIX)
        && key
            .chars()
            .all(|character| matches!(character, 'a'..='z' | 'A'..='Z' | '_' | '+'..='='))
}

/// Refuse a `Tags` map carrying a key outside the modelled form ([`TAG_KEY_PATTERN`]).
///
/// `Ok(())` when every key it carries is admitted, and for a map carrying none.
/// Otherwise the modelled `ValidationException` (HTTP 400), worded the way this service
/// words a constraint the request broke
/// ([`crate::service::map_key_pattern_refusal`]): one clause, at the map member, stating
/// the requirement its KEYS have to satisfy. One clause for one request however many
/// keys broke the rule, and whatever order the request's entries arrived in, exactly as
/// a reported collection states one requirement once.
///
/// The caller must apply this BEFORE writing anything, so a refused call attaches no tag
/// — which is what keeps a reserved key out of the set every later read reports.
pub fn tags_within_modelled_form(
    tags: &HashMap<String, String>,
) -> Result<(), ValidationException> {
    if tags.keys().all(|key| tag_key_within_modelled_form(key)) {
        return Ok(());
    }
    Err(validation(crate::service::map_key_pattern_refusal(
        TAGS_MEMBER_PATH,
        TAG_KEY_PATTERN,
    )))
}

/// Refuse a `tagKeys` list naming a key outside the modelled form ([`TAG_KEY_PATTERN`]) —
/// the SAME rule [`tags_within_modelled_form`] applies to the keys of a map, for the one
/// call that names keys in a list.
///
/// Stated at the LIST member with the requirement its ELEMENTS have to satisfy nested in
/// it and the whole list quoted ([`crate::service::element_pattern_refusal`]), which is
/// how this service states every constraint an element of a list breaks. The position
/// reported is the first offending element's, so one list yields one refusal whichever
/// of its keys broke the rule.
pub fn named_tag_keys_within_modelled_form(
    tag_keys: &[String],
) -> Result<(), ValidationException> {
    let offending = tag_keys
        .iter()
        .position(|key| !tag_key_within_modelled_form(key));
    match offending {
        // `position` is 0-based and a clause names a 1-based one.
        Some(index) => Err(validation(crate::service::element_pattern_refusal(
            TAG_KEYS_MEMBER_PATH,
            tag_keys,
            index + 1,
            TAG_KEY_PATTERN,
        ))),
        None => Ok(()),
    }
}

/// How many credentials this emulator will hold for ONE application.
///
/// Neither the model nor `UpdateApplicationSettings`' documentation states how many
/// credentials an application may hold — only how many ONE call may carry
/// (`ApplicationCredentialList` is `@length(min: 0, max: 20)`) — so this is the
/// emulator's own ceiling rather than a fidelity claim about a published quota: it is
/// what stops a caller looping on `UpdateApplicationSettings` with credentials for fresh
/// database names growing one application's record, and the database set derived from it
/// ([`StoredApplication::databases`]), without bound. Generous enough that no realistic
/// SAP system meets it, and reported with `ValidationException`, which every call that
/// writes a credential models.
pub const MAX_CREDENTIALS_PER_APPLICATION: usize = 100;

/// The grants held for ONE resource: `PermissionActionType` -> the source resource
/// ARNs permitted to take that action on it.
///
/// `BTreeMap`/`BTreeSet` rather than the hashed containers because the rendered policy
/// document's statement order has to be the same for every read of an unchanged
/// resource, and because a repeated grant of the same triple must be the same single
/// statement (the set is what makes `PutResourcePermission` idempotent).
pub type ResourceGrants = BTreeMap<String, BTreeSet<String>>;

/// The `Action` a statement names for `action_type`: this service's ARN namespace
/// (its IAM action prefix, [`ARN_SERVICE`]) and the `PermissionActionType` the grant
/// was made with, which is how an IAM action is spelled.
pub fn permission_policy_action(action_type: &str) -> String {
    format!("{ARN_SERVICE}:{action_type}")
}

/// A change the clock owes an application: at `due_at` the application takes `change`,
/// whether or not anybody called.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// When the change falls due. See [`TRANSITION_SPAN`].
    pub due_at: SystemTime,
    /// What the application becomes at `due_at`.
    pub change: PendingChange,
}

/// What the clock owes an application: the asynchronous calls of this service either
/// move the application to another status or settle the settings an update asked for.
/// A deregistration owes nothing, because it ends the registration as it is accepted
/// ([`RegionState::deregister_application`]).
#[derive(Debug, Clone)]
pub enum PendingChange {
    /// The application takes these settings — what an `UpdateApplicationSettings` call
    /// owes it.
    Settings(PendingSettings),
    /// The application takes these `ApplicationStatus`/`ApplicationDiscoveryStatus`
    /// values — what registering, starting and stopping owe it.
    Status {
        /// The `ApplicationStatus` the application takes when the change falls due.
        status: String,
        /// The `ApplicationDiscoveryStatus` it takes then.
        discovery_status: String,
        /// What the application EXPLAINS that outcome with when it falls due, and `None`
        /// for an outcome that explains itself.
        ///
        /// `Application$StatusMessage` explains a status the application reached, so the
        /// work that computed the refusal behind a non-success outcome is what states it:
        /// the call stores the sentence here with the status it owes, and
        /// [`StoredApplication::settle`] attaches it at the same instant it moves the
        /// status — exactly as the operation record of that same work carries its own
        /// explanation to its terminal status ([`StoredOperation::terminal_status_message`]).
        status_message: Option<String>,
    },
}

/// The settings an `UpdateApplicationSettings` call owes an application: what the
/// application's own settings become when the update falls due.
///
/// The update is asynchronous, so the call does not change the settings — it states what
/// they BECOME, and [`StoredApplication::settle`] is what makes them so, at the same
/// instant for every reader. A member left `None` is a setting the call did not name,
/// which the update leaves exactly as it was.
#[derive(Debug, Clone)]
pub struct PendingSettings {
    /// The credentials the application holds once the update falls due: the result of
    /// adding, updating and removing what the call named
    /// ([`StoredApplication::credentials_after`]), computed when the call was made
    /// because nothing else can change them while this update is outstanding.
    pub credentials: Vec<ApplicationCredential>,
    /// The AWS Backint Agent for SAP HANA configuration the call asked for, when it
    /// asked for one.
    pub backint: Option<BackintConfig>,
    /// The ARN of the SAP HANA database that replaces the application's current SAP HANA
    /// connection, when the call named one.
    pub database_arn: Option<String>,
}

/// A `DatabaseStatus` the clock owes the database management system connected to an
/// application: the status it takes at `due_at`, whether or not anybody calls.
#[derive(Debug, Clone)]
pub struct PendingDbmsStatus {
    /// When the change falls due. See [`TRANSITION_SPAN`].
    pub due_at: SystemTime,
    /// The `DatabaseStatus` the connected DBMS takes then.
    pub status: String,
}

/// The running state of the database management system an application is connected to:
/// the SAP HANA system its databases ([`StoredApplication::databases`]) live in.
///
/// It is held SEPARATELY from the application's own status because the two are started
/// and stopped separately: a stop of the application reaches the connected DBMS only
/// when the call names it as the connected entity to stop
/// (`StopApplication$StopConnectedEntity`), the way that call's other switch reaches the
/// Amazon EC2 instances only when it asks for them, while a start of the application
/// brings the DBMS up with it because the call carries no switch to leave it down. So
/// what `Database$Status` reports is this status rather than a second reading of the
/// application's ([`Self::status_for`]).
///
/// Stopping and starting a DBMS is asynchronous like every other unit of work here: the
/// call moves it to the transient status now and stores the instant the next one falls
/// due ([`PendingDbmsStatus`]), which [`Self::settle`] materialises for every reader at
/// once. Nothing sleeps.
#[derive(Debug, Clone)]
pub struct StoredDbms {
    /// A `DatabaseStatus` value; see the `DATABASE_STATUS_*` constants.
    pub status: String,
    /// The status the clock owes it, if one is outstanding.
    pub pending: Option<PendingDbmsStatus>,
}

impl StoredDbms {
    /// The DBMS of a freshly registered application: a registration describes a running
    /// SAP system, so nothing has stopped its database system. While the registration
    /// itself is still under way no database status follows from it at all, which is
    /// what [`Self::status_for`] reports.
    pub fn running() -> Self {
        Self {
            status: DATABASE_STATUS_RUNNING.to_string(),
            pending: None,
        }
    }

    /// Materialise the status the clock already owes this DBMS, and answer the instant it
    /// was owed at — `None` when nothing was due.
    ///
    /// The change was due at [`PendingDbmsStatus::due_at`] whether or not anybody called,
    /// so applying it on a read reports what the DBMS already is rather than changing it.
    pub fn settle(&mut self, now: SystemTime) -> Option<SystemTime> {
        let due_at = self.pending.as_ref().map(|pending| pending.due_at)?;
        if now < due_at {
            return None;
        }
        let pending = self.pending.take().expect("checked present just above");
        self.status = pending.status;
        Some(due_at)
    }

    /// Stop the DBMS: it is `STOPPING` now and `STOPPED` one [`TRANSITION_SPAN`] later.
    pub fn stop(&mut self, now: SystemTime) {
        self.transition(now, DATABASE_STATUS_STOPPING, DATABASE_STATUS_STOPPED);
    }

    /// Start the DBMS: it is `STARTING` now and `RUNNING` one [`TRANSITION_SPAN`] later.
    pub fn start(&mut self, now: SystemTime) {
        self.transition(now, DATABASE_STATUS_STARTING, DATABASE_STATUS_RUNNING);
    }

    /// Put the DBMS in `transient` now and owe it `settled` one [`TRANSITION_SPAN`] from
    /// now, so a caller acting straight after the call still observes the transition.
    fn transition(&mut self, now: SystemTime, transient: &str, settled: &str) {
        self.status = transient.to_string();
        self.pending = Some(PendingDbmsStatus {
            due_at: now + TRANSITION_SPAN,
            status: settled.to_string(),
        });
    }

    /// The `DatabaseStatus` a database of an application reports, given the
    /// `ApplicationStatus` that application's running state follows from
    /// ([`StoredApplication::established_status`]) — `None` where the service has
    /// established none.
    ///
    /// A database lives in the connected DBMS, so the status it reports is that DBMS's —
    /// but only where the service has established what the application runs at all. Where
    /// it has not (the registration is still under way, or its discovery never succeeded)
    /// nothing is known about that application's databases, whatever this record says the
    /// DBMS is, which is what the model's `UNKNOWN` states.
    pub fn status_for(&self, established_status: Option<&str>) -> &str {
        if established_status.is_some() {
            self.status.as_str()
        } else {
            DATABASE_STATUS_UNKNOWN
        }
    }
}

/// A registered SAP application: the state `Application` and `ApplicationSummary` are
/// rebuilt from.
#[derive(Debug, Clone)]
pub struct StoredApplication {
    pub id: String,
    /// An `ApplicationType` value (`HANA` | `SAP_ABAP`), exactly as registered.
    pub app_type: String,
    pub arn: String,
    /// The ARN of the AWS Service Catalog AppRegistry application that records this
    /// registration ([`app_registry_arn`]).
    ///
    /// The service records every registered application in AppRegistry, so the ARN is
    /// assigned when the application is registered and held for as long as it exists:
    /// it is reported in `Application$AppRegistryArn` and is one of the references
    /// `GetApplication` resolves an application by
    /// ([`RegionState::resolve_application_id`]). Not optional, because there is no
    /// registered application without one.
    ///
    /// Minted per registration ([`new_app_registry_application_id`]) and never
    /// reassigned, so it is also the identity of THIS registration, as opposed to any
    /// other made under the same `ApplicationId`: that is what the records of the calls
    /// made on this application are filed against ([`StoredOperation::registration`]).
    pub app_registry_arn: String,
    /// An `ApplicationStatus` value; see the `APPLICATION_STATUS_*` constants.
    pub status: String,
    /// An `ApplicationDiscoveryStatus` value; see the `DISCOVERY_STATUS_*` constants.
    pub discovery_status: String,
    /// What this application EXPLAINS its current status with, and `None` for a status
    /// that explains itself.
    ///
    /// `Application$StatusMessage` explains the status the application is in, so it is
    /// written with that status: the work whose outcome moves the application computes
    /// the sentence and stores it on the change it owes
    /// ([`PendingChange::Status::status_message`]), and [`Self::settle`] attaches it at
    /// the instant the status becomes the application's.
    pub status_message: Option<String>,
    /// The components the registration DESCRIBES for this application's SAP system
    /// ([`registered_components`]): one per `ComponentsInfo` entry, plus the SAP HANA
    /// component of the SAP system its `Sid` names.
    ///
    /// This is the landscape the service's discovery of this application has to find, NOT
    /// what the application already has: a component is the application's only once
    /// discovery has produced it, which is what [`Self::components`] answers and what
    /// every reader goes through. Nothing else reads this field.
    pub described_components: Vec<StoredComponent>,
    /// Whether the service's discovery of this application has completed successfully.
    ///
    /// Registration is asynchronous and discovery runs as part of it, so an application
    /// whose registration is still under way has discovered nothing yet; the first
    /// `SUCCESS` discovery result the clock owes it ([`Self::settle`]) is what completes
    /// it. A later re-discovery (`StartApplicationRefresh`) re-runs over the same
    /// landscape, so it neither hides the components already discovered nor adds others.
    pub discovery_completed: bool,
    pub last_updated: SystemTime,
    pub sid: Option<String>,
    pub sap_instance_number: Option<String>,
    /// The EC2 instances the registration named.
    pub instances: Vec<String>,
    pub credentials: Vec<ApplicationCredential>,
    /// The ARN of the SAP HANA database the registration named, when it named one: one
    /// of the databases of this application ([`StoredApplication::databases`]). An
    /// `UpdateApplicationSettings` call replaces it with the database it names.
    pub database_arn: Option<String>,
    /// The AWS Backint Agent for SAP HANA configuration this application holds, and
    /// `None` for one no call has configured the agent for.
    ///
    /// Nothing registers it: the configuration is `UpdateApplicationSettings$Backint`'s
    /// to install, so a freshly registered application holds none, and a later update
    /// that names a configuration is updating one it already holds rather than
    /// installing it ([`settings_update_message`] states which).
    pub backint: Option<BackintConfig>,
    /// The running state of the database management system this application is connected
    /// to — the SAP HANA system its databases live in, started and stopped separately
    /// from the application itself ([`StoredDbms`]).
    pub dbms: StoredDbms,
    /// The change the clock owes this application, if one is outstanding.
    pub pending: Option<PendingTransition>,
}

impl StoredApplication {
    /// Materialise the change the clock already owes this application — the next status
    /// an asynchronous call moves it to, or the settings an update asked for.
    ///
    /// The change was due at [`PendingTransition::due_at`] whether or not anybody
    /// called, so applying it on a read reports what the resource already is rather
    /// than changing it — and `LastUpdated` is stamped with the due instant, not with
    /// the time of the call that happened to observe it.
    pub fn settle(&mut self, now: SystemTime) {
        // The connected DBMS runs on its own clock: a stop that named it as the connected
        // entity, or a start that brought it back up, is due whether or not the
        // application has a change outstanding too.
        if let Some(due_at) = self.dbms.settle(now) {
            self.last_updated = due_at.max(self.last_updated);
        }
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        let pending = self.pending.take().expect("checked present just above");
        match pending.change {
            PendingChange::Status {
                status,
                discovery_status,
                status_message,
            } => {
                // A discovery result of `SUCCESS` is what completes the discovery of this
                // application, and therefore what makes the landscape its registration
                // described its components ([`Self::components`]): before that result
                // nothing has been discovered for it, so it has none.
                if discovery_status == DISCOVERY_STATUS_SUCCESS {
                    self.discovery_completed = true;
                }
                self.status = status;
                self.discovery_status = discovery_status;
                // `StatusMessage` explains the status the application is IN, so it is
                // written with that status and not beside it: the outcome the work
                // computed carries its explanation here, and an outcome that explains
                // itself leaves the application explaining nothing rather than keeping
                // the sentence of a status it is no longer in.
                self.status_message = status_message;
            }
            PendingChange::Settings(settings) => {
                self.credentials = settings.credentials;
                // A setting the update did not name is left exactly as it was, which is
                // what the absent member means.
                if let Some(backint) = settings.backint {
                    self.backint = Some(backint);
                }
                if let Some(database_arn) = settings.database_arn {
                    // The named database REPLACES the application's current SAP HANA
                    // connection, so it is this application's database from here on
                    // ([`Self::databases`]).
                    self.database_arn = Some(database_arn);
                }
            }
        }
        self.last_updated = pending.due_at.max(self.last_updated);
    }

    /// The credentials this application holds after an update that adds or updates
    /// `add_or_update` and removes `remove` — what
    /// `UpdateApplicationSettings$CredentialsToAddOrUpdate` and `CredentialsToRemove`
    /// ask for.
    ///
    /// A credential is identified by the SAP HANA database it reaches and its
    /// `CredentialType`, by the same identity a database itself has here
    /// ([`database_id_from_name`]), so a credential named for the database and type of
    /// one this application already holds REPLACES it (the secret the new one names is
    /// the one that reaches that database from then on) and one naming a database and
    /// type it holds none for is ADDED, after the ones it keeps ([`credentials_with`], the
    /// one place a list of credential declarations is applied, so two declarations naming
    /// the same credential leave one credential here too). Removal drops the credential of
    /// that database and type.
    ///
    /// Refuses with the modelled `ValidationException` (HTTP 400), and computes nothing,
    /// when the call asks for a change the application cannot take: a credential named
    /// for removal that it does not hold, one named in BOTH lists, whose two
    /// requests contradict each other, and one that would leave the application holding
    /// more than [`MAX_CREDENTIALS_PER_APPLICATION`] credentials. The caller applies this
    /// BEFORE writing anything, so a refused update leaves the credentials it holds
    /// untouched.
    pub fn credentials_after(
        &self,
        add_or_update: &[ApplicationCredential],
        remove: &[ApplicationCredential],
    ) -> Result<Vec<ApplicationCredential>, ValidationException> {
        for removed in remove {
            if add_or_update
                .iter()
                .any(|added| same_credential(added, removed))
            {
                return Err(validation(format!(
                    "The {} credential for database {} is named both for update and for \
                     removal in the same request",
                    removed.credential_type, removed.database_name
                )));
            }
            if !self
                .credentials
                .iter()
                .any(|held| same_credential(held, removed))
            {
                return Err(validation(format!(
                    "Application {} has no {} credential for database {}",
                    self.id, removed.credential_type, removed.database_name
                )));
            }
        }
        let kept: Vec<ApplicationCredential> = self
            .credentials
            .iter()
            .filter(|held| !remove.iter().any(|removed| same_credential(held, removed)))
            .cloned()
            .collect();
        // Only a credential of an identity the set holds none of grows it; one of an
        // identity already there — held before the call, or declared earlier in this very
        // list — is replaced in place ([`credentials_with`]), so what the call would leave
        // the application holding is the length of that result. The emulator's own
        // per-application ceiling is what stops a caller looping with fresh database names
        // growing this record without bound.
        let credentials = credentials_with(&kept, add_or_update);
        let would_hold = credentials.len();
        if would_hold > MAX_CREDENTIALS_PER_APPLICATION {
            return Err(validation(format!(
                "this emulator holds at most {MAX_CREDENTIALS_PER_APPLICATION} credentials per application, but this request would leave {} holding {would_hold}",
                self.id
            )));
        }
        Ok(credentials)
    }

    /// Whether the service's registration of this application COMPLETED successfully:
    /// its discovery ran and succeeded, so the service manages the SAP system the
    /// registration described.
    ///
    /// This is the precondition every call that acts on the managed system has: a
    /// registration that is still under way has discovered nothing yet and one that
    /// failed never will, and in both cases there is nothing for such a call to act on
    /// ([`not_registered_successfully`]).
    pub fn registered_successfully(&self) -> bool {
        self.discovery_completed
    }

    /// Whether the registration of this application ENDED without completing: its
    /// discovery could not be carried out, which is the terminal state a failed
    /// registration converges to ([`DISCOVERY_STATUS_REGISTRATION_FAILED`]).
    ///
    /// Distinct from [`Self::registered_successfully`] being false, which a registration
    /// still under way also is: this one is settled and will not become successful, so a
    /// call that needs a working application reports that the registration is failed
    /// rather than that work is in flight ([`failed_registration_message`]).
    pub fn registration_failed(&self) -> bool {
        self.discovery_status == DISCOVERY_STATUS_REGISTRATION_FAILED
    }

    /// The `ApplicationStatus` the running state of this application's SAP system follows
    /// from, and `None` where the service has established none.
    ///
    /// Two things have to hold for the status to state what the application runs. The
    /// service has to have DISCOVERED the application ([`Self::registered_successfully`]):
    /// a registration still under way has found nothing yet and one that ended without
    /// completing never will, so in neither case is there a system whose running state
    /// anything could be reported from. And the status itself has to state a running state
    /// at all — the application is up, or on its way up or down.
    ///
    /// The status ALONE is not that evidence, which is why both conditions are asked here:
    /// a stop of this service is served against an application whose registration is still
    /// under way ([`application_control_available`] gates work in flight, not the
    /// registration outcome, and a stop does not wait for a running registration), so an
    /// application whose discovery then fails also reaches the transient and settled
    /// statuses of that stop while carrying its failed discovery forward. Every reading of
    /// something INSIDE the application goes through this ([`StoredDbms::status_for`],
    /// [`component_status_for`]), so none of them reports a running state the service never
    /// established.
    pub fn established_status(&self) -> Option<&str> {
        if !self.registered_successfully() {
            return None;
        }
        match self.status.as_str() {
            status @ (APPLICATION_STATUS_ACTIVATED
            | APPLICATION_STATUS_STARTING
            | APPLICATION_STATUS_STOPPING
            | APPLICATION_STATUS_STOPPED) => Some(status),
            _ => None,
        }
    }

    /// The `ApplicationDiscoveryStatus` this application is GOING to report: the one the
    /// change the clock owes it carries, or the one it already reports when that change
    /// carries none.
    ///
    /// A control call that supersedes an outstanding status change carries this forward
    /// rather than the status the application reports right now, so the result of a
    /// discovery that was already running — the one its registration started — is not
    /// lost by a call made while it was still under way.
    pub fn owed_discovery_status(&self) -> &str {
        match self.pending.as_ref().map(|pending| &pending.change) {
            Some(PendingChange::Status {
                discovery_status, ..
            }) => discovery_status.as_str(),
            _ => self.discovery_status.as_str(),
        }
    }

    /// What the application is GOING to explain its status with: the explanation the
    /// change the clock owes it carries, or the one it already reports when that change
    /// carries none.
    ///
    /// The counterpart of [`Self::owed_discovery_status`], for the same reason: a control
    /// call that supersedes an outstanding status change carries the outcome of the work
    /// already running forward, so the sentence explaining that outcome is carried forward
    /// with it rather than dropped.
    pub fn owed_status_message(&self) -> Option<String> {
        match self.pending.as_ref().map(|pending| &pending.change) {
            Some(PendingChange::Status { status_message, .. }) => status_message.clone(),
            _ => self.status_message.clone(),
        }
    }

    /// Whether the change the clock owes this application can leave it deriving a
    /// DIFFERENT set of resources once that change is materialised at `now`.
    ///
    /// The SAP HANA databases of an application are DERIVED from its credentials and its
    /// SAP HANA connection ([`Self::databases`]), and the settings an update settles
    /// rewrite both — so a database the application stops deriving stops existing at that
    /// instant, exactly as every resource of a registration does when the registration
    /// ends. [`RegionState::settle`] asks this before materialising the change, because
    /// what is held ON such a database has to go with it ([`RegionState::forget_resource`]).
    pub fn derived_resources_change_due(&self, now: SystemTime) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            now >= pending.due_at && matches!(pending.change, PendingChange::Settings(_))
        })
    }

    /// The Amazon EC2 instances this application runs on, each once, in the order the
    /// registration named them.
    ///
    /// A registration associates an application with an EC2 instance in EITHER of two
    /// places, exactly as it describes a component ([`registered_components`]): the
    /// `Instances` the SAP system itself runs on, and the `Ec2InstanceId` each declared
    /// component runs on. Both are instances of this application, so both are what a
    /// call that acts on "the associated Amazon EC2 instance" acts on; an id named twice
    /// is one instance, so it is answered once.
    pub fn ec2_instance_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let declared = self
            .instances
            .iter()
            .chain(self.components().iter().map(|c| &c.ec2_instance_id));
        for id in declared {
            if id.is_empty() || ids.iter().any(|known| known == id) {
                continue;
            }
            ids.push(id.clone());
        }
        ids
    }

    /// Project the stored application onto the modelled `Application`.
    ///
    /// `associated_application_arns` are the ARNs of the applications this one is
    /// associated with. That is a relation BETWEEN registrations rather than anything one
    /// of them holds, so it is resolved against the partition the application is read out
    /// of ([`RegionState::associated_application_arns`]) and handed in here — which is why
    /// handlers project an application through [`RegionState::application_view`] rather
    /// than calling this directly.
    pub fn view(&self, associated_application_arns: Vec<String>) -> Application {
        Application {
            id: Some(self.id.clone()),
            r#type: Some(self.app_type.clone()),
            arn: Some(self.arn.clone()),
            app_registry_arn: Some(self.app_registry_arn.clone()),
            status: Some(self.status.clone()),
            discovery_status: Some(self.discovery_status.clone()),
            components: Some(self.components().iter().map(|c| c.id.clone()).collect()),
            last_updated: Some(::aws_smithy_types::DateTime::from(self.last_updated)),
            status_message: self.status_message.clone(),
            associated_application_arns: if associated_application_arns.is_empty() {
                None
            } else {
                Some(associated_application_arns)
            },
        }
    }

    /// Project the stored application onto the modelled `ApplicationSummary`, with the
    /// tags held for its ARN.
    pub fn summary(&self, tags: Option<HashMap<String, String>>) -> ApplicationSummary {
        ApplicationSummary {
            id: Some(self.id.clone()),
            discovery_status: Some(self.discovery_status.clone()),
            r#type: Some(self.app_type.clone()),
            arn: Some(self.arn.clone()),
            tags,
        }
    }

    /// The value a `ListApplications` filter whose `Name` is `name` compares against,
    /// or `None` when this application has no such attribute.
    ///
    /// The names are the members the model gives `ApplicationSummary` — `Id`,
    /// `DiscoveryStatus`, `Type`, `Arn` — which is the shape `ListApplications`
    /// returns, so a filter names something the response itself reports.
    pub fn filter_attribute(&self, name: &str) -> Option<&str> {
        match name {
            "Id" => Some(self.id.as_str()),
            "DiscoveryStatus" => Some(self.discovery_status.as_str()),
            "Type" => Some(self.app_type.as_str()),
            "Arn" => Some(self.arn.as_str()),
            _ => None,
        }
    }

    /// The SAP HANA databases of this application, in `DatabaseId` order.
    ///
    /// The emulator discovers nothing on the registered instances, so the databases an
    /// application has are the ones its registration named — and a registration names a
    /// database in EITHER of two places, so the set is the union of both:
    ///
    /// * `DatabaseArn` names the application's SAP HANA database by ARN, whose last
    ///   segment is the database's own name ([`database_name_from_arn`]); the ARN the
    ///   registration gave it is that database's ARN, so it is the ARN reported for it
    ///   rather than one minted here. It has to NAME a database to name one
    ///   ([`arn_names_a_database`]): an ARN of this service that names a resource of
    ///   another kind names no database of this or of any other application.
    /// * every `ApplicationCredential` names, in `DatabaseName`, the SAP HANA database
    ///   its secret reaches; a database no ARN named is a sub-resource of this
    ///   application, so its ARN is minted from the two ids ([`database_arn`]).
    ///
    /// A database is identified by its name ([`database_id_from_name`]), so two of those
    /// mentions naming the same database are ONE database — and an application whose
    /// registration named none, by ARN or by credential, has none.
    ///
    /// `region` is the Region the application is registered in, which is the Region the
    /// ARNs minted here carry.
    pub fn databases(&self, region: &str) -> Vec<ApplicationDatabase> {
        // The component a HANA database belongs to, READ OFF the components this
        // application has rather than derived a second time, so a `ComponentId` reported
        // for a database is one `GetComponent` resolves.
        let component_id = self.hana_component_id().map(str::to_string);
        // Keyed by `DatabaseId`, which both deduplicates and puts the databases in id
        // order, so a page taken over them is stable between calls.
        let mut databases: BTreeMap<String, ApplicationDatabase> = BTreeMap::new();
        // The database the registration named by ARN goes in FIRST, so that when a
        // credential names that same database the ARN the registration gave it is the
        // one reported for it.
        let named_by_arn = self
            .database_arn
            .as_deref()
            .filter(|arn| arn_names_a_database(arn))
            .and_then(|arn| database_name_from_arn(arn).map(|name| (arn, name)));
        if let Some((arn, database_name)) = named_by_arn {
            let database_id = database_id_from_name(database_name);
            databases.insert(
                database_id.clone(),
                ApplicationDatabase {
                    application_id: self.id.clone(),
                    component_id: component_id.clone(),
                    arn: arn.to_string(),
                    database_id,
                    database_name: database_name.to_string(),
                    database_type: database_type_for(database_name).to_string(),
                },
            );
        }
        for credential in &self.credentials {
            let database_id = database_id_from_name(&credential.database_name);
            databases
                .entry(database_id.clone())
                .or_insert_with(|| ApplicationDatabase {
                    application_id: self.id.clone(),
                    component_id: component_id.clone(),
                    arn: database_arn(region, &self.id, &database_id),
                    database_id,
                    database_name: credential.database_name.clone(),
                    database_type: database_type_for(&credential.database_name).to_string(),
                });
        }
        databases.into_values().collect()
    }

    /// The `ComponentId` of the SAP HANA component of this application's own SAP system,
    /// or `None` when it has no such component.
    ///
    /// It is the component this application's SAP HANA databases belong to
    /// ([`Self::databases`]): the one identified by the System ID the registration named
    /// and the SAP HANA component type ([`component_id`]), which a registration naming a
    /// System ID describes ([`registered_components`]). Resolved THROUGH the component
    /// set, so the id a database reports is one [`Self::component`] answers and
    /// [`Self::view`] lists, and the two cannot drift apart.
    pub fn hana_component_id(&self) -> Option<&str> {
        let sid = self.sid.as_deref()?;
        let id = component_id(sid, COMPONENT_TYPE_HANA);
        self.component(&id).map(|component| component.id.as_str())
    }

    /// The component of this application whose `ComponentId` is `component_id`, or `None`
    /// when the application has no such component.
    ///
    /// The components an application has are the ones its discovery produced
    /// ([`Self::components`]), so this resolves exactly the ids [`Self::view`] reports in
    /// `Application$Components`.
    pub fn component(&self, component_id: &str) -> Option<&StoredComponent> {
        self.components()
            .iter()
            .find(|component| component.id == component_id)
    }

    /// The components the service has DISCOVERED for this application — the ones every
    /// reader of a component reports, resolves and keeps tags and permissions under.
    ///
    /// A component of a registered application exists only once the service's discovery of
    /// that application has produced it, so an application whose discovery has not
    /// completed has NONE, whatever its registration described
    /// ([`Self::described_components`]): what a registration declares is a description of
    /// the landscape to discover, not an inventory of components. Once discovery has
    /// completed the application has the components that landscape holds, and keeps them
    /// for as long as the application exists — a re-discovery runs over the same
    /// registration, and a deregistration ends them with the application itself.
    pub fn components(&self) -> &[StoredComponent] {
        if self.discovery_completed {
            &self.described_components
        } else {
            &[]
        }
    }

    /// Project `component` of this application onto the modelled `Component`.
    ///
    /// What the response can state about a component is what the registration declared
    /// about it (its id, System ID, type and EC2 instance) plus what this store knows
    /// about the application it belongs to: the status its components are in, the instant
    /// the application last changed, and the SAP HANA databases of the application that
    /// belong to this component ([`Self::databases`], which is also what `ListDatabases`
    /// reports for it). Everything the real service reads off the instance by discovery —
    /// hostnames, SAP features, kernel and HANA versions, system replication and cluster
    /// state, connection endpoints — is left ABSENT rather than invented, because the
    /// emulator discovers nothing.
    ///
    /// `region` is the Region the application is registered in, which is the Region the
    /// ARNs reported here carry.
    pub fn component_view(&self, component: &StoredComponent, region: &str) -> Component {
        // The databases of the application that belong to THIS component, by the same
        // (database -> component) assignment `ListDatabases` reports and narrows by.
        let databases: Vec<String> = self
            .databases(region)
            .into_iter()
            .filter(|database| database.component_id.as_deref() == Some(component.id.as_str()))
            .map(|database| database.database_id)
            .collect();
        Component {
            component_id: Some(component.id.clone()),
            sid: Some(component.sid.clone()),
            // The SAP system number of the component: the instance number the
            // registration named for the application's SAP system, which is this
            // component's only when it was declared under that same System ID.
            system_number: self
                .sap_instance_number
                .clone()
                .filter(|_| self.sid.as_deref() == Some(component.sid.as_str())),
            // `ParentComponent`/`ChildComponents` describe a highly available setup.
            // Nothing registers a parent or child relation between components, so a
            // component declared here has neither.
            parent_component: None,
            child_components: None,
            application_id: Some(self.id.clone()),
            component_type: Some(component.component_type.clone()),
            status: Some(component_status_for(self.established_status()).to_string()),
            // Discovered on the instance; see this method's docs.
            sap_hostname: None,
            sap_feature: None,
            sap_kernel_version: None,
            hdb_version: None,
            resilience: None,
            // The EC2 instance the registration declared this component on is the host it
            // is associated with; the rest of the host's properties are discovered.
            associated_host: Some(AssociatedHost {
                hostname: None,
                ec2_instance_id: Some(component.ec2_instance_id.clone()),
                ip_addresses: None,
                os_version: None,
            }),
            databases: if databases.is_empty() {
                None
            } else {
                Some(databases)
            },
            // `Hosts` and `PrimaryHost` are deprecated in the model in favour of
            // `AssociatedHost`, so nothing is reported in them.
            hosts: None,
            primary_host: None,
            database_connection: None,
            last_updated: Some(::aws_smithy_types::DateTime::from(self.last_updated)),
            arn: Some(component_arn(region, &self.id, &component.id)),
        }
    }

    /// The ARNs of every resource this registration gives rise to: the application's own
    /// ARN, the ARNs of the SAP HANA databases derived from it ([`Self::databases`]) and
    /// the ARNs of its components ([`component_arn`]) — each one a resource this service
    /// reports an ARN for and keeps tags and resource permissions under, keyed by exactly
    /// that spelling.
    ///
    /// This is the ONE enumeration of a registration's resources, so the resolver that
    /// decides whether an ARN names something ([`RegionState::holds_resource`]) and the
    /// cleanup that drops what was held per resource when that resource goes
    /// ([`RegionState::forget_destroyed`]) cannot disagree about which resources a
    /// registration has.
    ///
    /// `region` is the Region the application is registered in, which is the Region the
    /// ARNs listed here carry.
    pub fn resource_arns(&self, region: &str) -> Vec<String> {
        let mut arns = ::std::vec![self.arn.clone()];
        arns.extend(self.databases(region).into_iter().map(|db| db.arn));
        arns.extend(
            self.components()
                .iter()
                .map(|component| component_arn(region, &self.id, &component.id)),
        );
        arns
    }

    /// Project `database` of this application onto the modelled `Database`.
    ///
    /// The stored half is the same [`ApplicationDatabase`] `ListDatabases` projects onto
    /// `DatabaseSummary` ([`ApplicationDatabase::summary`]), so a database read one by one
    /// and the same database in a listing report the same ids, type and ARN. What a
    /// `Database` states BEYOND that summary is, again, what the registration declared
    /// plus what this store knows about the application the database belongs to:
    ///
    /// * `Credentials` are the registration's `ApplicationCredential`s that name THIS
    ///   database ([`Self::databases`] identifies a database by its name, so the same
    ///   match decides which credentials reach it); a database no credential names
    ///   carries none rather than the application's other credentials.
    /// * `Status` follows the application's, as a component's does
    ///   ([`StoredDbms::status_for`]), which a stop that names the connected DBMS moves
    ///   without moving the application's.
    /// * `LastUpdated` is the instant the application record last changed, which is the
    ///   last thing that could have changed its databases.
    /// * `ConnectedComponentArns` are the ARNs of this application's components that this
    ///   database belongs to — the same (database -> component) assignment
    ///   `Component$Databases` ([`Self::component_view`]) and `ListDatabases` report, so
    ///   the two cannot disagree about which component a database is connected to.
    ///
    /// `PrimaryHost` and `SQLPort` are read off the HANA instance by discovery, which the
    /// emulator does not do, so they are left ABSENT rather than invented — the same
    /// choice [`Self::component_view`] makes for the properties it would have to discover.
    ///
    /// `region` is the Region the application is registered in, which is the Region the
    /// ARNs reported here carry.
    pub fn database_view(&self, database: &ApplicationDatabase, region: &str) -> Database {
        let credentials: Vec<ApplicationCredential> = self
            .credentials
            .iter()
            .filter(|credential| {
                database_id_from_name(&credential.database_name) == database.database_id
            })
            .cloned()
            .collect();
        let connected_component_arns: Vec<String> = self
            .components()
            .iter()
            .filter(|component| database.component_id.as_deref() == Some(component.id.as_str()))
            .map(|component| component_arn(region, &self.id, &component.id))
            .collect();
        Database {
            application_id: Some(self.id.clone()),
            component_id: database.component_id.clone(),
            credentials: if credentials.is_empty() {
                None
            } else {
                Some(credentials)
            },
            database_id: Some(database.database_id.clone()),
            database_name: Some(database.database_name.clone()),
            database_type: Some(database.database_type.clone()),
            arn: Some(database.arn.clone()),
            status: Some(self.dbms.status_for(self.established_status()).to_string()),
            // Discovered on the HANA instance; see this method's docs.
            primary_host: None,
            sql_port: None,
            last_updated: Some(::aws_smithy_types::DateTime::from(self.last_updated)),
            connected_component_arns: if connected_component_arns.is_empty() {
                None
            } else {
                Some(connected_component_arns)
            },
        }
    }
}

/// A component of a registered application: the record `Component` is projected from by
/// [`StoredApplication::component_view`].
///
/// The emulator discovers nothing on the registered instances, so a component is what the
/// registration described about it ([`registered_components`]) — the type it runs, the
/// System ID of the SAP system it belongs to, and the EC2 instance it runs on.
#[derive(Debug, Clone)]
pub struct StoredComponent {
    /// The `ComponentId`, derived from the System ID and the type by [`component_id`].
    pub id: String,
    /// A `ComponentType` value, exactly as described (see
    /// [`crate::ops::types::COMPONENT_TYPE_VALUES`]).
    pub component_type: String,
    /// The `SID` of the SAP system the component belongs to.
    pub sid: String,
    /// The `Ec2InstanceId` the component runs on.
    pub ec2_instance_id: String,
}

impl StoredComponent {
    /// Project this component onto the modelled `ComponentSummary`, with its own ARN and
    /// the tags held for that ARN.
    ///
    /// A summary states what identifies the component — the application it belongs to,
    /// its own id and the type it was declared as — and its ARN, which names both ids
    /// ([`component_arn`]) because a `ComponentId` is unique only within its
    /// application. The ARN is passed in rather than rebuilt here, so the caller that
    /// looked the tags up by it reports the very same spelling. Everything else a
    /// `Component` carries is read by `GetComponent`
    /// ([`StoredApplication::component_view`]); a summary does not carry it.
    pub fn summary(
        &self,
        application_id: &str,
        arn: &str,
        tags: Option<HashMap<String, String>>,
    ) -> ComponentSummary {
        ComponentSummary {
            application_id: Some(application_id.to_string()),
            component_id: Some(self.id.clone()),
            component_type: Some(self.component_type.clone()),
            tags,
            arn: Some(arn.to_string()),
        }
    }

    /// The key a component listing is ordered and paged by: the id of the application
    /// this component belongs to and its own id, in that order ([`composite_page_key`],
    /// which is what makes ordering these keys as strings the same as ordering by
    /// (application id, component id)).
    ///
    /// `application_id` is passed in because a `ComponentId` is unique only within its
    /// application, so a component record does not name the application on its own.
    pub fn page_key(&self, application_id: &str) -> String {
        composite_page_key(application_id, &self.id)
    }
}

/// A SAP HANA database of a registered application: the record `DatabaseSummary` is
/// projected from.
///
/// DERIVED from the registration by [`StoredApplication::databases`] rather than stored
/// beside it, so the registration stays the single source of truth for its databases and
/// no call can report one the application does not have.
#[derive(Debug, Clone)]
pub struct ApplicationDatabase {
    /// The `ApplicationId` of the application this database belongs to.
    pub application_id: String,
    /// The `ComponentId` of the application's HANA component, absent when the
    /// registration named no System ID to identify that component by.
    pub component_id: Option<String>,
    /// The `DatabaseId`, derived from the name by [`database_id_from_name`].
    pub database_id: String,
    /// The `DatabaseName` the registration named this database by: a credential's
    /// `DatabaseName`, or the name the registration's `DatabaseArn` ends with.
    pub database_name: String,
    /// A `DatabaseType` value; see [`database_type_for`].
    pub database_type: String,
    /// The ARN of the database: the one the registration's `DatabaseArn` gave it when it
    /// named this database by ARN, and otherwise the sub-resource ARN minted from the
    /// application and database ids ([`database_arn`]).
    pub arn: String,
}

impl ApplicationDatabase {
    /// Project the database onto the modelled `DatabaseSummary`, with the tags held for
    /// its ARN.
    pub fn summary(&self, tags: Option<HashMap<String, String>>) -> DatabaseSummary {
        DatabaseSummary {
            application_id: Some(self.application_id.clone()),
            component_id: self.component_id.clone(),
            database_id: Some(self.database_id.clone()),
            database_type: Some(self.database_type.clone()),
            arn: Some(self.arn.clone()),
            tags,
        }
    }

    /// The key a database listing is ordered and paged by: this database's application
    /// id and its own id, in that order ([`composite_page_key`], which is what makes
    /// ordering these keys as strings the same as ordering by (application id, database
    /// id)).
    pub fn page_key(&self) -> String {
        composite_page_key(&self.application_id, &self.database_id)
    }
}

/// A persisted asynchronous operation: the record `GetOperation` reads.
#[derive(Debug, Clone)]
pub struct StoredOperation {
    pub id: String,
    /// An `OperationType` value; see the `OPERATION_TYPE_*` constants.
    pub op_type: String,
    /// An `OperationStatus` value; see the `OPERATION_STATUS_*` constants.
    pub status: String,
    pub status_message: Option<String>,
    /// The parameters of the call that filed this operation, if it carried any.
    pub properties: HashMap<String, String>,
    pub resource_type: String,
    pub resource_id: String,
    pub resource_arn: String,
    /// WHICH registration this record was filed against: the ARN of the AppRegistry
    /// application assigned when that application was registered
    /// ([`StoredApplication::app_registry_arn`]). Not a modelled member — `Operation`
    /// reports the resource, not the registration — and nothing but the join below reads
    /// it.
    ///
    /// An `ApplicationId` is free again the instant a deregistration completes, and the
    /// ARNs of this service are a pure function of the ids they embed, so neither the id
    /// nor the ARN this record names tells the registration it was filed against apart
    /// from a later one made under the same id. The AppRegistry application is minted per
    /// registration and never reassigned, so this does — which is what lets a listing
    /// scoped to one application report that application's OWN records alone
    /// ([`RegionState::operations_for`]).
    pub registration: String,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub last_updated_time: SystemTime,
    /// When this operation's terminal status falls due. See [`TRANSITION_SPAN`].
    pub terminal_due_at: SystemTime,
    /// The `OperationStatus` this operation reaches at [`Self::terminal_due_at`]: the
    /// outcome of the work it was filed for.
    ///
    /// An operation of this service is asynchronous, so its outcome is settled when the
    /// work it tracks is — which for the registration that files one is whether the
    /// discovery it runs can be carried out at all. The outcome is therefore STORED on
    /// the record when it is filed and materialised by the clock ([`Self::settle`]),
    /// never decided by the read that observes it.
    pub terminal_status: String,
    /// What the record explains that terminal status with, absent for an outcome that
    /// explains itself: `StatusMessage` explains a non-success status, so this is the
    /// refusal the work ran into.
    pub terminal_status_message: Option<String>,
    /// The resources BESIDES the one the call named that this operation acted on, in the
    /// order it acted on them; see [`OperationTarget`]. Empty for a call that acted on
    /// its own resource alone.
    pub targets: Vec<OperationTarget>,
}

/// One resource OTHER than the one the call named that an operation acted on: the record
/// the milestone of that work is derived from ([`StoredOperation::events`]).
///
/// An operation of this service can reach past the application the call named — the
/// instance shutdown `StopApplication$IncludeEc2InstanceShutdown` asks for
/// ([`ec2_shutdown::stop_instances`]) acts on an Amazon EC2 instance — and such a
/// resource is part of the work the operation was asked to do, so the operation reports
/// reaching it. One record per resource acted on, holding that resource's OWN identity
/// (its ARN, its id and its `Resource$ResourceType`) rather than the application's, and
/// the outcome the service that owns it answered with — never a status for work that was
/// never asked or never answered.
#[derive(Debug, Clone)]
pub struct OperationTarget {
    /// The id of the resource, which is what a `resourceID` filter compares against.
    pub resource_id: String,
    /// Its `Resource$ResourceArn`.
    pub resource_arn: String,
    /// Its `Resource$ResourceType`: `AWS::<Service>::<Resource>`, as the
    /// `OperationEventResourceType` pattern (`^[\w]+::[\w]+::[\w]+$`) describes. See
    /// [`OPERATION_EVENT_RESOURCE_TYPE_EC2_INSTANCE`].
    pub resource_type: String,
    /// `OperationEvent$Description`: what the operation asked of this resource, worded
    /// the way the model's own example words it ("Stop the EC2 instance i-…").
    pub description: String,
    /// When the operation asked for that work.
    pub asked_at: SystemTime,
    /// What the service that owns the resource REFUSED the work with, absent when it
    /// accepted it. A refusal is a milestone that failed, and it is reported as the
    /// failure it was rather than as work that completed.
    pub refusal: Option<String>,
}

impl OperationTarget {
    /// The `OperationEventStatus` of this resource's milestone, what explains it and when
    /// it was reached, given the status of the operation it belongs to.
    ///
    /// The work was asked for as part of that operation, so — unless the owning service
    /// refused it, which is a failure at the instant it answered — it is under way while
    /// the operation is and is done when the operation is done.
    fn milestone(
        &self,
        operation_status: &str,
        operation_ended_at: SystemTime,
        operation_status_message: Option<String>,
    ) -> (&'static str, Option<String>, SystemTime) {
        if let Some(refusal) = &self.refusal {
            return (
                OPERATION_EVENT_STATUS_FAILED,
                Some(refusal.clone()),
                self.asked_at,
            );
        }
        match operation_status {
            OPERATION_STATUS_SUCCESS => (OPERATION_EVENT_STATUS_COMPLETED, None, operation_ended_at),
            OPERATION_STATUS_ERROR => (
                OPERATION_EVENT_STATUS_FAILED,
                operation_status_message,
                operation_ended_at,
            ),
            // Work that is still running has no status to explain.
            _ => (OPERATION_EVENT_STATUS_IN_PROGRESS, None, self.asked_at),
        }
    }
}

impl StoredOperation {
    /// How this record ranks for eviction when the store is full: see [`evict_oldest`].
    /// The record with the smallest rank goes first, so a finished record is dropped
    /// before any that is still running, oldest first.
    fn eviction_rank(&self) -> (bool, SystemTime) {
        (self.status == OPERATION_STATUS_INPROGRESS, self.start_time)
    }
    /// Materialise the terminal status the clock already owes this operation: it was
    /// due to reach [`Self::terminal_status`] at [`Self::terminal_due_at`] whether or not
    /// anybody asked.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != OPERATION_STATUS_INPROGRESS || now < self.terminal_due_at {
            return;
        }
        self.status = self.terminal_status.clone();
        self.status_message = self.terminal_status_message.clone();
        self.end_time = Some(self.terminal_due_at);
        self.last_updated_time = self.terminal_due_at.max(self.last_updated_time);
    }

    /// The milestones this operation has reached, oldest first: the events
    /// `ListOperationEvents` reports for it.
    ///
    /// DERIVED from this record rather than held beside it, the way an application's
    /// databases are derived from its registration ([`StoredApplication::databases`]):
    /// every record starts `INPROGRESS`, so its first milestone is the start of its work,
    /// and a record the clock has brought to a terminal status has a last milestone at
    /// the instant it reached it — `COMPLETED` for a success and `FAILED` for an error,
    /// carrying whatever that record explains its status with. A milestone therefore
    /// exists exactly because the operation passed it, and the events of an operation
    /// still running move to its terminal milestone at the same instant
    /// [`Self::settle`] moves its status.
    ///
    /// Between those two, ONE milestone per resource the operation acted on besides the
    /// one the call named ([`Self::targets`]): the work an operation was asked to do can
    /// reach a resource of another kind, or of another service, and each such resource is
    /// a milestone of this operation, naming that resource's own ARN, id and
    /// `Resource$ResourceType` and carrying the outcome the service that owns it answered
    /// with ([`OperationTarget::milestone`]). The order is the order the work happened
    /// in, which is also what the ordinals a page resumes at count
    /// ([`OperationEventRecord::page_key`]).
    pub fn events(&self) -> Vec<OperationEventRecord> {
        // The resource kind as a `Resource$ResourceType`, and as the word a description
        // names it by — the record's own kind either way, so neither is written out per
        // operation type.
        let resource_type = event_resource_type(&self.resource_type).map(str::to_string);
        let kind = self.resource_type.to_lowercase();
        let milestone = |ordinal: usize, description: String, status: &str, status_message, timestamp| {
            OperationEventRecord {
                ordinal,
                description,
                resource_arn: self.resource_arn.clone(),
                resource_id: self.resource_id.clone(),
                resource_type: resource_type.clone(),
                status: status.to_string(),
                status_message,
                timestamp,
            }
        };
        let mut events = vec![milestone(
            0,
            format!("{} started for {kind} {}", self.op_type, self.resource_id),
            OPERATION_EVENT_STATUS_IN_PROGRESS,
            // `StatusMessage` explains a status; work that has just begun has nothing
            // to explain.
            None,
            self.start_time,
        )];
        // A record that reached a terminal status reached it at its `EndTime`; the
        // fallback is for one that carries none, whose last change is the most this
        // record knows about when that status was taken.
        let ended_at = self.end_time.unwrap_or(self.last_updated_time);
        for target in &self.targets {
            let (status, status_message, timestamp) =
                target.milestone(&self.status, ended_at, self.status_message.clone());
            events.push(OperationEventRecord {
                ordinal: events.len(),
                description: target.description.clone(),
                resource_arn: target.resource_arn.clone(),
                resource_id: target.resource_id.clone(),
                resource_type: Some(target.resource_type.clone()),
                status: status.to_string(),
                status_message,
                timestamp,
            });
        }
        let terminal = if self.status == OPERATION_STATUS_SUCCESS {
            Some((OPERATION_EVENT_STATUS_COMPLETED, "completed"))
        } else if self.status == OPERATION_STATUS_ERROR {
            Some((OPERATION_EVENT_STATUS_FAILED, "failed"))
        } else {
            None
        };
        if let Some((status, verb)) = terminal {
            events.push(milestone(
                events.len(),
                format!("{} {verb} for {kind} {}", self.op_type, self.resource_id),
                status,
                self.status_message.clone(),
                ended_at,
            ));
        }
        events
    }

    /// The value a `ListOperations` filter whose `Name` is `name` compares against, or
    /// `None` when this record has no such attribute (which nothing matches).
    ///
    /// The names are the identifying and status members the model gives `Operation` —
    /// `Id`, `Type`, `Status`, `ResourceType`, `ResourceId`, `ResourceArn` — which is the
    /// shape that listing returns, so a filter names something the response itself
    /// reports, the same way [`StoredConfigurationCheckOperation::filter_attribute`] does
    /// for the configuration check listing. See [`OPERATION_FILTER_NAMES`].
    pub fn filter_attribute(&self, name: &str) -> Option<&str> {
        match name {
            "Id" => Some(self.id.as_str()),
            "Type" => Some(self.op_type.as_str()),
            "Status" => Some(self.status.as_str()),
            "ResourceType" => Some(self.resource_type.as_str()),
            "ResourceId" => Some(self.resource_id.as_str()),
            "ResourceArn" => Some(self.resource_arn.as_str()),
            _ => None,
        }
    }

    /// Project the stored operation onto the modelled `Operation`.
    pub fn view(&self) -> Operation {
        Operation {
            id: Some(self.id.clone()),
            r#type: Some(self.op_type.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            properties: if self.properties.is_empty() {
                None
            } else {
                Some(self.properties.clone())
            },
            resource_type: Some(self.resource_type.clone()),
            resource_id: Some(self.resource_id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            last_updated_time: Some(::aws_smithy_types::DateTime::from(self.last_updated_time)),
        }
    }
}

/// The `Resource$ResourceType` spelling of an operation record's own `ResourceType`, and
/// `None` for a resource kind this service has no such spelling for.
///
/// The two members are different strings for the same thing: `Operation$ResourceType` is
/// unconstrained while `OperationEventResourceType` carries a pattern, so a kind with no
/// spelling here is reported ABSENT by [`StoredOperation::events`] rather than as a value
/// that pattern refuses.
pub fn event_resource_type(resource_type: &str) -> Option<&'static str> {
    (resource_type == APPLICATION_RESOURCE_TYPE)
        .then_some(OPERATION_EVENT_RESOURCE_TYPE_APPLICATION)
}

/// How many digits an event's ordinal is rendered with in its listing key: see
/// [`OperationEventRecord::page_key`]. An operation has one milestone per status it has
/// passed plus one per resource it acted on besides the one the call named — at most
/// [`MAX_COMPONENTS_PER_APPLICATION`] Amazon EC2 instances of one application — so no
/// record comes close to this many.
const EVENT_PAGE_KEY_WIDTH: usize = 4;

/// One milestone of an operation: the record `ListOperationEvents` reports as an
/// `OperationEvent`.
///
/// Derived from the [`StoredOperation`] it belongs to by [`StoredOperation::events`], so
/// it lives and dies with that record.
#[derive(Debug, Clone)]
pub struct OperationEventRecord {
    /// Which milestone of its operation this is, counting from zero in the order they
    /// happen — the key the listing is ordered and paged by ([`Self::page_key`]).
    pub ordinal: usize,
    /// `OperationEvent$Description`: what the milestone was.
    pub description: String,
    /// The `ResourceArn` of the resource the operation acted on.
    pub resource_arn: String,
    /// The id of that resource, which is what a `resourceID` filter compares against.
    pub resource_id: String,
    /// Its `Resource$ResourceType`; see [`event_resource_type`] for when it is absent.
    pub resource_type: Option<String>,
    /// An `OperationEventStatus` value; see the `OPERATION_EVENT_STATUS_*` constants.
    pub status: String,
    /// What the operation record explains that status with, when it explains it.
    pub status_message: Option<String>,
    /// When the operation reached this milestone.
    pub timestamp: SystemTime,
}

impl OperationEventRecord {
    /// Project the milestone onto the modelled `OperationEvent`.
    pub fn view(&self) -> OperationEvent {
        OperationEvent {
            description: Some(self.description.clone()),
            resource: Some(Resource {
                resource_arn: Some(self.resource_arn.clone()),
                resource_type: self.resource_type.clone(),
            }),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            timestamp: Some(::aws_smithy_types::DateTime::from(self.timestamp)),
        }
    }

    /// The value a `ListOperationEvents` filter whose `Name` is `name` compares against,
    /// or `None` when this milestone has no such attribute (which nothing matches).
    ///
    /// The names are the ones `ListOperationEvents` documents for its `Filters`
    /// ([`OPERATION_EVENT_FILTER_NAMES`]), each naming something the event itself
    /// reports.
    pub fn filter_attribute(&self, name: &str) -> Option<&str> {
        match name {
            "status" => Some(self.status.as_str()),
            "resourceID" => Some(self.resource_id.as_str()),
            "resourceType" => self.resource_type.as_deref(),
            _ => None,
        }
    }

    /// The key this listing is ordered and paged by: the milestone's ordinal, zero-padded
    /// to [`EVENT_PAGE_KEY_WIDTH`] digits so ordering the keys as strings is ordering the
    /// milestones by when they happened — which is what makes a `NextToken` resume at the
    /// milestone the previous page stopped at, including across the later call at which a
    /// terminal milestone has fallen due.
    pub fn page_key(&self) -> String {
        format!("{:0width$}", self.ordinal, width = EVENT_PAGE_KEY_WIDTH)
    }
}

/// `RuleResultStatus` of a rule the registered configuration satisfies. One of the
/// model's `RuleResultStatus` values (see
/// [`crate::ops::types::RULE_RESULT_STATUS_VALUES`]).
pub const RULE_RESULT_STATUS_PASSED: &str = "PASSED";

/// `RuleResultStatus` of a rule the registered configuration does not satisfy.
pub const RULE_RESULT_STATUS_FAILED: &str = "FAILED";

/// `RuleResultStatus` of a rule the registered configuration satisfies only in part, so
/// the system runs but not as the rule asks.
pub const RULE_RESULT_STATUS_WARNING: &str = "WARNING";

/// `RuleResultStatus` of a rule that has nothing to judge in this configuration — the
/// subject it is about is not part of the system the application registered.
pub const RULE_RESULT_STATUS_INFO: &str = "INFO";

/// `RuleResultStatus` of a rule whose outcome is not known: the registration does not
/// state the property the rule is about, so neither a pass nor a failure can be
/// reported for it. The model documents this value as "the number of rules with unknown
/// status" in `RuleStatusCounts$Unknown`.
pub const RULE_RESULT_STATUS_UNKNOWN: &str = "UNKNOWN";

/// Everything Systems Manager for SAP holds in ONE Region: the applications registered
/// there, the operation records filed there, their tags, and the resource permissions
/// granted there.
///
/// The service is regional, so a registration lives in the account AND the Region it was
/// made in: that is why every application ARN embeds a Region
/// ([`application_arn`]) and why this crate's refusals state the account and Region the
/// lookup ran in. The store is therefore partitioned per Region and a handler reaches
/// ONLY the partition of the Region its request was signed for
/// ([`ComAmazonawsSsmsapSsmsapState::region`] /
/// [`ComAmazonawsSsmsapSsmsapState::region_entry`]): nothing registered at one regional
/// endpoint is listed, read, controlled or deregistered through another, and an
/// `ApplicationId` in use in one Region is free in every other.
///
/// `applications` and `operations` are `BTreeMap`s rather than `HashMap`s because
/// `ListApplications` pages over the first: iteration order has to be the same for the
/// call that hands out a `NextToken` and the call that resumes from it, and id order is
/// that stable order.
#[derive(Debug, Default)]
pub struct RegionState {
    /// Every application registered in this Region, keyed by `ApplicationId`.
    pub applications: BTreeMap<String, StoredApplication>,
    /// Every operation record filed in this Region, keyed by `OperationId`. Bounded by
    /// [`MAX_RETAINED_OPERATIONS`].
    pub operations: BTreeMap<String, StoredOperation>,
    /// Every configuration check operation, keyed by its own `OperationId` — written by
    /// `StartConfigurationChecks` and read by `GetConfigurationCheckOperation`. A
    /// separate map from `operations` because the two are separate resources in the
    /// model: one is read as an `Operation`, the other as a
    /// `ConfigurationCheckOperation`. Bounded by [`MAX_RETAINED_OPERATIONS`] too.
    pub configuration_check_operations: BTreeMap<String, StoredConfigurationCheckOperation>,
    /// Tags by resource ARN — written by `RegisterApplication`'s `Tags` and read by
    /// `GetApplication`/`ListApplications`, so the later tagging APIs
    /// (`TagResource`/`UntagResource`/`ListTagsForResource`) have one place to join.
    /// An application ARN embeds its Region, so these keys are this Region's ARNs.
    pub tags_by_arn: HashMap<String, HashMap<String, String>>,
    /// The resource permission grants held for each target resource ARN, written by
    /// `PutResourcePermission` and read by `GetResourcePermission`, so both report one
    /// policy for a resource rather than two copies of it.
    ///
    /// A `BTreeMap` for the same reason as [`ResourceGrants`]: the policy document a
    /// read renders has to be byte-identical between reads of an unchanged resource.
    pub resource_permissions: BTreeMap<String, ResourceGrants>,
}

impl RegionState {
    /// Materialise every change the clock already owes, on applications and on
    /// operations alike — the status an asynchronous call moves an application to, and
    /// the resources a settling change stops deriving.
    ///
    /// Every handler calls this first, so each one reports the same thing about a
    /// resource at the same instant.
    ///
    /// The work is LINEAR in what this partition holds, whatever a caller does: each
    /// application is settled once, an application whose settling change can rewrite the
    /// resources derived from it has those enumerated twice, and the ARNs that stopped
    /// existing are then resolved against ONE enumeration of the partition's remaining
    /// resources rather than one per ARN. So the whole pass costs at most a few
    /// enumerations of the applications this Region holds ([`APPLICATIONS_QUOTA`]) and the
    /// resources each registration gives rise to, and never their product, however many
    /// changes fall due in the same instant.
    pub fn settle(&mut self, now: SystemTime) {
        // The ARNs of resources that have STOPPED existing at `now`, gathered by the pass
        // below and resolved ONCE at the end ([`Self::forget_destroyed`]): what is held on
        // each of them is forgotten there, so the pass pays to re-resolve no ARN and the
        // whole cleanup costs one enumeration of this partition's resources, and only when
        // something was destroyed at all.
        let mut destroyed: Vec<String> = Vec::new();

        // A change that settles can DESTROY a resource derived from a registration
        // without ending the registration: the databases of an application are derived
        // from its credentials and its SAP HANA connection
        // ([`StoredApplication::databases`]), both of which the settings an update
        // settles rewrite, so a database the application stops deriving stops existing at
        // that instant. Which ARNs those are is read off the ONE enumeration of a
        // registration's resources ([`StoredApplication::resource_arns`]), diffed across
        // the change — and only for an application whose settling change can rewrite that
        // set, so every other settle costs what it always did.
        for application in self.applications.values_mut() {
            let region = arn_region(&application.arn).to_string();
            let before = application
                .derived_resources_change_due(now)
                .then(|| application.resource_arns(&region));
            application.settle(now);
            if let Some(before) = before {
                let after = application.resource_arns(&region);
                destroyed.extend(before.into_iter().filter(|arn| !after.contains(arn)));
            }
        }
        self.forget_destroyed(destroyed);
        for operation in self.operations.values_mut() {
            operation.settle(now);
        }
        // A configuration check run reaches its terminal status at its own instant too,
        // and what it reports then is the EVALUATION of its check's rules against the
        // application it was started for ([`ConfigurationCheckEntry::evaluate`]) — so the
        // rules are evaluated here, at the instant the run was due to finish whether or
        // not anybody polled it, and never by the read that observes the record.
        let due: Vec<String> = self
            .configuration_check_operations
            .iter()
            .filter(|(_, run)| run.terminal_due(now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(run) = self.configuration_check_operations.get(&id) else {
                continue;
            };
            let (application_id, check_id) = (run.application_id.clone(), run.check_id.clone());
            // The application is resolved here rather than held on the run, so the rules
            // judge the configuration as it stands when the run ends.
            let evaluated = match self.applications.get(&application_id) {
                Some(application) => configuration_check_entry(&check_id)
                    .map(|entry| entry.evaluate(application, arn_region(&application.arn)))
                    .ok_or_else(|| uncatalogued_configuration_check_message(&check_id)),
                None => Err(configuration_check_subject_gone_message(&application_id)),
            };
            if let Some(run) = self.configuration_check_operations.get_mut(&id) {
                run.settle(now, evaluated);
            }
        }
    }

    /// End the registration of `application_id` in this Region, and answer the record it
    /// held — `None` when this Region holds no application of that id.
    ///
    /// The deregistration is COMPLETE when it is accepted: from that instant the
    /// application resolves for nobody ([`Self::resolve_application_id`]), so every later
    /// read and every later mutation of that id answers the condition that the account has
    /// no such registered application, each in its own error family
    /// ([`no_registered_application`], [`application_not_found_message`]). The call does
    /// not report a status for an application that no longer exists, and there is no window
    /// in which one caller sees it gone and another still sees it.
    ///
    /// What was held FOR that registration goes with it: its tags and the resource
    /// permissions granted on it, both keyed by its ARN, and the configuration check runs
    /// that inspected it, which describe a configuration that no longer exists. What is
    /// held PER RESOURCE goes for every resource of the registration — the application's
    /// own ARN and the ARNs of the resources DERIVED from it
    /// ([`StoredApplication::resource_arns`]) — since a database or a component cannot
    /// outlive the registration it came from and neither can what was tagged or granted on
    /// it.
    ///
    /// Its `Operation` records STAY: they are the history of calls already made, which
    /// `GetOperation` resolves by id and [`MAX_RETAINED_OPERATIONS`] bounds — and they stay
    /// the records of the registration they were filed against, which has ended, so no
    /// later registration made under the same id reports them as its own
    /// ([`Self::operations_for`]).
    pub fn deregister_application(&mut self, application_id: &str) -> Option<StoredApplication> {
        let application = self.applications.remove(application_id)?;
        let destroyed = application.resource_arns(arn_region(&application.arn));
        self.forget_configuration_checks_for(application_id);
        self.forget_destroyed(destroyed);
        Some(application)
    }

    /// Forget what this Region holds on each ARN in `destroyed` that no longer names a
    /// resource of it.
    ///
    /// The ONE place the ARNs of resources that stopped existing are resolved, so the two
    /// ways a resource goes away — a registration that ends
    /// ([`Self::deregister_application`]) and a settling change that stops deriving one of
    /// its resources ([`Self::settle`]) — cost the same single pass and cannot disagree.
    ///
    /// The ARNs this partition STILL holds are enumerated once: the same enumeration
    /// [`Self::holds_resource`] resolves an ARN through, so a destroyed ARN is one no call
    /// that takes an ARN resolves any more. It is asked of the set rather than of the
    /// resolver per ARN because one DATABASE can be named by more than one registration
    /// (`RegisterApplication$DatabaseArn` and `UpdateApplicationSettings$DatabaseArn` name
    /// a database that need not be a resource of the registration naming it,
    /// [`arn_names_a_database`]), so a database another registration still names must keep
    /// what is held on it — and one pass over this partition answers that for every
    /// destroyed ARN at once.
    fn forget_destroyed(&mut self, destroyed: Vec<String>) {
        if destroyed.is_empty() {
            return;
        }
        let live: BTreeSet<String> = self
            .applications
            .values()
            .flat_map(|application| application.resource_arns(arn_region(&application.arn)))
            .collect();
        for arn in &destroyed {
            if !live.contains(arn) {
                self.forget_resource(arn);
            }
        }
    }

    /// Forget everything this Region holds ON the resource `arn` names, because that
    /// resource has stopped existing.
    ///
    /// Tags and resource permissions are held PER RESOURCE, keyed by exactly the ARN the
    /// resource is reported under, so neither can outlive the resource it was attached to:
    /// once the ARN names nothing, no tag set is held for it and no policy is rendered
    /// from grants keyed by it. The ARNs of this service are a pure function of the ids
    /// they embed, so a resource later derived under the SAME ARN is a new resource that
    /// carries neither — which is what this guarantees. It is also what lets the partition
    /// be reclaimed once nothing observable is left in it ([`Self::is_empty`]).
    ///
    /// The ONE place that cleanup lives, so the two ways a resource stops existing — a
    /// registration that ends, and a settling change that stops deriving one of its
    /// resources — cannot disagree about what goes with it.
    fn forget_resource(&mut self, arn: &str) {
        self.tags_by_arn.remove(arn);
        self.resource_permissions.remove(arn);
    }

    /// Whether this Region holds nothing at all — the state an unused partition is in,
    /// and the one a partition returns to when its last application is deregistered.
    pub fn is_empty(&self) -> bool {
        self.applications.is_empty()
            && self.operations.is_empty()
            && self.configuration_check_operations.is_empty()
            && self.tags_by_arn.is_empty()
            && self.resource_permissions.is_empty()
    }

    /// Resolve the application a request names by its id, its ARN, or its AppRegistry
    /// ARN — the three members `GetApplication` accepts — to an `ApplicationId` stored
    /// IN THIS REGION.
    ///
    /// So a reference to an application of another Region resolves to nothing here,
    /// including an ARN: an application ARN embeds the Region it was registered in, so
    /// it cannot match a record of this one.
    pub fn resolve_application_id(
        &self,
        application_id: Option<&str>,
        application_arn: Option<&str>,
        app_registry_arn: Option<&str>,
    ) -> Option<String> {
        if let Some(id) = application_id {
            return self.applications.get(id).map(|app| app.id.clone());
        }
        if let Some(arn) = application_arn {
            return self
                .applications
                .values()
                .find(|app| app.arn == arn)
                .map(|app| app.id.clone());
        }
        if let Some(arn) = app_registry_arn {
            // The AppRegistry application a registration is recorded in is assigned to
            // that application alone ([`StoredApplication::app_registry_arn`]), so its
            // ARN names exactly one application of this Region.
            return self
                .applications
                .values()
                .find(|app| app.app_registry_arn == arn)
                .map(|app| app.id.clone());
        }
        None
    }

    /// Project `application` onto the modelled `Application` as this partition holds it.
    ///
    /// The ONE place an `Application` is assembled, so every call that answers one —
    /// `RegisterApplication` and `GetApplication` — reports the same thing about the same
    /// application, including the applications it is associated with
    /// ([`Self::associated_application_arns`]), which no single record can answer alone.
    pub fn application_view(&self, application: &StoredApplication) -> Application {
        application.view(self.associated_application_arns(application))
    }

    /// The ARNs of the applications of this Region that `application` is ASSOCIATED with,
    /// in `ApplicationId` order.
    ///
    /// Two registered applications are associated when they share an SAP HANA database:
    /// an SAP_ABAP application's SAP HANA connection names the database of the HANA
    /// application that runs it (`RegisterApplication$DatabaseArn` names it at
    /// registration and `UpdateApplicationSettings$DatabaseArn` replaces it later), so
    /// the database is one of BOTH applications' databases
    /// ([`StoredApplication::databases`]) and each is an associated application of the
    /// other. The relation is therefore derived from the same database set every
    /// `ListDatabases` and `GetDatabase` resolves, matched on the database's own ARN as
    /// [`Self::resolve_database`] matches one — so it is symmetric, it follows whichever
    /// call last set that connection, and it ends when either registration does, with
    /// nothing to keep in step on either record.
    ///
    /// An application is never associated with itself, and one sharing no database with
    /// any other is associated with none.
    ///
    /// The work is ONE enumeration of this partition's applications and of the databases
    /// each registration derives, so what a read costs is bounded by the store's own
    /// ceilings ([`APPLICATIONS_QUOTA`], [`MAX_CREDENTIALS_PER_APPLICATION`]) rather than by
    /// anything a request carries.
    pub fn associated_application_arns(&self, application: &StoredApplication) -> Vec<String> {
        let databases: BTreeSet<String> = application
            .databases(arn_region(&application.arn))
            .into_iter()
            .map(|database| database.arn)
            .collect();
        if databases.is_empty() {
            return Vec::new();
        }
        self.applications
            .values()
            .filter(|other| other.id != application.id)
            .filter(|other| {
                other
                    .databases(arn_region(&other.arn))
                    .iter()
                    .any(|database| databases.contains(&database.arn))
            })
            .map(|other| other.arn.clone())
            .collect()
    }

    /// The tags held for `arn`, absent when none are.
    pub fn tags_for(&self, arn: &str) -> Option<HashMap<String, String>> {
        self.tags_by_arn
            .get(arn)
            .filter(|tags| !tags.is_empty())
            .cloned()
    }

    /// Attach `tags` to the resource `arn` names, in the one place this service keeps
    /// tags ([`Self::tags_by_arn`]), so every later read reports them.
    ///
    /// Tagging MERGES into what the resource already holds: a key it does not hold is
    /// added, a key it holds takes the value this call carries, and a key this call
    /// does not name keeps the value it had. So tagging the same pair twice leaves ONE
    /// tag and the second call changes nothing, and a call that carries no tags at all
    /// leaves the set exactly as it was.
    ///
    /// Refused with [`validation`] when the resource would be left holding more than
    /// [`MAX_TAGS_PER_RESOURCE`] tags; a refused call writes nothing.
    pub fn apply_tags(
        &mut self,
        arn: &str,
        tags: &HashMap<String, String>,
    ) -> Result<(), ValidationException> {
        if tags.is_empty() {
            // Nothing to attach, so nothing is written — not even an empty entry for a
            // resource no tag is held for.
            return Ok(());
        }
        let held = self.tags_by_arn.get(arn);
        // Only a key the resource does not already hold grows the set; a key it holds
        // is overwritten in place.
        let added = tags
            .keys()
            .filter(|key| held.is_none_or(|held| !held.contains_key(*key)))
            .count();
        let would_hold = held.map_or(0, HashMap::len) + added;
        if would_hold > MAX_TAGS_PER_RESOURCE {
            return Err(validation(format!(
                "this emulator holds at most {MAX_TAGS_PER_RESOURCE} tags per resource, but this request would leave {arn} holding {would_hold}"
            )));
        }
        let held = self.tags_by_arn.entry(arn.to_string()).or_default();
        for (key, value) in tags {
            held.insert(key.clone(), value.clone());
        }
        Ok(())
    }

    /// Remove the tags `tag_keys` names from the set held for the resource `arn` names,
    /// in the one place this service keeps tags ([`Self::tags_by_arn`]), so every later
    /// read reports the set without them.
    ///
    /// Only the keys NAMED are removed: a key the resource holds and this call does not
    /// name keeps its value. A key the resource does not hold removes nothing, so the
    /// call is idempotent — removing the same keys twice leaves the same set — and
    /// naming no key at all leaves the set exactly as it was.
    ///
    /// Once nothing is left held for `arn`, its entry goes too, which is what an
    /// untagged resource is stored as (so an emptied resource counts no differently
    /// from one nothing ever tagged, see [`Self::is_empty`]).
    pub fn remove_tags(&mut self, arn: &str, tag_keys: &[String]) {
        let Some(held) = self.tags_by_arn.get_mut(arn) else {
            // Nothing is held for the resource, so there is nothing any key could
            // remove — and no empty entry is minted for it.
            return;
        };
        for key in tag_keys {
            held.remove(key);
        }
        if held.is_empty() {
            self.tags_by_arn.remove(arn);
        }
    }

    /// Whether `arn` names a resource THIS Region holds.
    ///
    /// The resources AWS Systems Manager for SAP holds are the registered SAP application
    /// and the resources derived from that registration, so an ARN resolves here when it
    /// is a registered application's own ARN ([`StoredApplication::arn`]) or the ARN of
    /// one of the resources derived from it ([`StoredApplication::resource_arns`]: the SAP
    /// HANA databases and the components of the registration) — and nothing else, since
    /// the registration is the only thing this store holds. Every resource this service
    /// reports an ARN for therefore resolves here, which is what makes the ARN a response
    /// hands a caller usable in the calls that take one.
    ///
    /// `region` is the Region this partition holds, which is the Region those ARNs
    /// embed: an ARN of another Region therefore resolves to nothing here, exactly as
    /// an application of another Region does ([`Self::resolve_application_id`]).
    pub fn holds_resource(&self, region: &str, arn: &str) -> bool {
        // An application is resolved from its ARN the ONE way this store resolves one,
        // and that pass costs a string comparison per application; only when it finds
        // nothing are the derived resources ENUMERATED, which is the one place that
        // enumeration lives ([`StoredApplication::resource_arns`]).
        if self.resolve_application_id(None, Some(arn), None).is_some() {
            return true;
        }
        self.applications.values().any(|application| {
            application
                .resource_arns(region)
                .iter()
                .any(|held| held == arn)
        })
    }

    /// Whether `arn` names a SAP HANA DATABASE this Region holds.
    ///
    /// Narrower than [`Self::holds_resource`] on purpose: the resource-permission calls
    /// are database-scoped, so the ARN they take has to name one of the databases the
    /// registrations of this Region named ([`Self::resolve_database`], the one resolution
    /// this crate does for a database named by ARN, which is also what `GetDatabase`
    /// reads and `ListDatabases` reports) — an application's own ARN or a component's
    /// resolves nothing here even though this Region holds that resource.
    ///
    /// `region` is the Region this partition holds, which is the Region those ARNs embed,
    /// so a database of another Region resolves to nothing here.
    pub fn holds_database(&self, region: &str, arn: &str) -> bool {
        self.resolve_database(region, None, None, None, Some(arn))
            .is_some()
    }

    /// The SAP HANA database a request names, resolved to the application it belongs to
    /// and the database record itself.
    ///
    /// A database is named either by its own ARN (`database_arn`) or by its `DatabaseId`,
    /// and `application_id` / `component_id` NARROW where the lookup runs: an application
    /// the request names must hold the database, a component it names must be the one the
    /// database belongs to. The databases searched are the ones the registrations named
    /// ([`StoredApplication::databases`], which is also what `ListDatabases` reports), in
    /// the same (application id, database id) order that listing walks, so a reference two
    /// applications of this Region both answer resolves to the same one on every call. A
    /// `DatabaseId` names the database whose id it is by the identity a database has here
    /// ([`same_database_id`]), under which the case of a HANA database name is nothing.
    ///
    /// `None` when nothing this Region holds answers the reference — including a reference
    /// to a database of another Region, since the lookup only ever reaches this partition.
    /// A request that names NEITHER an ARN nor a database id names no database at all and
    /// resolves to nothing here (see
    /// [`MISSING_DATABASE_IDENTIFIER_MESSAGE`]).
    ///
    /// `region` is the Region this partition holds, which is the Region the ARNs the
    /// databases carry embed.
    pub fn resolve_database(
        &self,
        region: &str,
        application_id: Option<&str>,
        component_id: Option<&str>,
        database_id: Option<&str>,
        database_arn: Option<&str>,
    ) -> Option<(&StoredApplication, ApplicationDatabase)> {
        if database_arn.is_none() && database_id.is_none() {
            return None;
        }
        // An `ApplicationId` the request names narrows the lookup to that application,
        // which must be registered HERE; one that names none reaches every application of
        // this Region, in id order.
        let applications: Vec<&StoredApplication> = match application_id {
            Some(application_id) => ::std::vec![self.applications.get(application_id)?],
            None => self.applications.values().collect(),
        };
        applications.into_iter().find_map(|application| {
            application
                .databases(region)
                .into_iter()
                .find(|database| {
                    database_arn.is_none_or(|arn| database.arn == arn)
                        && database_id.is_none_or(|id| same_database_id(&database.database_id, id))
                        && component_id
                            .is_none_or(|id| database.component_id.as_deref() == Some(id))
                })
                .map(|database| (application, database))
        })
    }

    /// Permit `source_resource_arn` to take `action_type` on `resource_arn`.
    ///
    /// Adding a grant that is already held changes nothing — the grant is the
    /// (resource, action type, source) triple, so a repeated `PutResourcePermission`
    /// leaves one statement rather than a duplicate — and `Ok(false)` reports that
    /// nothing was added. Refuses with the modelled `ValidationException` at the
    /// store ceilings ([`MAX_PERMISSIONED_RESOURCES`],
    /// [`MAX_PERMISSION_SOURCES_PER_RESOURCE`]), and writes nothing when it refuses.
    pub fn grant_resource_permission(
        &mut self,
        resource_arn: &str,
        action_type: &str,
        source_resource_arn: &str,
    ) -> Result<bool, ValidationException> {
        let held_for_resource = self.resource_permissions.get(resource_arn);
        let held_sources = held_for_resource.and_then(|grants| grants.get(action_type));
        if held_sources.is_some_and(|sources| sources.contains(source_resource_arn)) {
            return Ok(false);
        }
        if held_for_resource.is_none()
            && self.resource_permissions.len() >= MAX_PERMISSIONED_RESOURCES
        {
            return Err(validation(format!(
                "this emulator holds permissions for at most {MAX_PERMISSIONED_RESOURCES} resources per Region"
            )));
        }
        if held_sources.map_or(0, BTreeSet::len) >= MAX_PERMISSION_SOURCES_PER_RESOURCE {
            return Err(validation(format!(
                "this emulator holds at most {MAX_PERMISSION_SOURCES_PER_RESOURCE} {action_type} permissions on one resource"
            )));
        }
        self.resource_permissions
            .entry(resource_arn.to_string())
            .or_default()
            .entry(action_type.to_string())
            .or_default()
            .insert(source_resource_arn.to_string());
        Ok(true)
    }

    /// Stop permitting the grants `resource_arn` holds, narrowed by the members the
    /// caller named: `action_type` to that `PermissionActionType` alone, and
    /// `source_resource_arn` to that one source. A call that narrows by neither
    /// removes every permission held on the resource.
    ///
    /// Revoking what is not held removes nothing and is not a failure: the grant is the
    /// (resource, action type, source) triple a set holds at most once, so removing it
    /// twice leaves the same state as removing it once. A (resource, action type) pair
    /// left with no source, and a resource left with no pair, are DROPPED rather than
    /// kept as an empty container: an empty entry would render no statement anyway
    /// ([`Self::resource_policy`]) while still counting against
    /// [`MAX_PERMISSIONED_RESOURCES`] and keeping the Region's partition alive.
    pub fn revoke_resource_permission(
        &mut self,
        resource_arn: &str,
        action_type: Option<&str>,
        source_resource_arn: Option<&str>,
    ) {
        let Some(grants) = self.resource_permissions.get_mut(resource_arn) else {
            return;
        };
        grants.retain(|action, sources| {
            if action_type.is_some_and(|wanted| wanted != action.as_str()) {
                // An action type the caller did not name keeps every source it holds.
                return true;
            }
            match source_resource_arn {
                Some(source) => {
                    sources.remove(source);
                }
                None => sources.clear(),
            }
            !sources.is_empty()
        });
        if grants.is_empty() {
            self.resource_permissions.remove(resource_arn);
        }
    }

    /// The resource policy document held for `resource_arn`, narrowed to
    /// `action_type` when the caller names one, and `None` when no grant matches —
    /// a resource nobody has granted anything on carries no policy, and nothing is
    /// fabricated in its place.
    ///
    /// The document is PROJECTED from the stored grants, one `Allow` statement per
    /// (action type, source resource) pair in the stable order the containers hold
    /// them, so two reads of an unchanged resource render the same bytes and the
    /// statement a grant added disappears exactly when the grant does.
    pub fn resource_policy(&self, resource_arn: &str, action_type: Option<&str>) -> Option<String> {
        let grants = self.resource_permissions.get(resource_arn)?;
        let statements: Vec<::serde_json::Value> = grants
            .iter()
            .filter(|(action, _)| action_type.is_none_or(|wanted| wanted == action.as_str()))
            .flat_map(|(action, sources)| {
                let action = permission_policy_action(action);
                sources.iter().map(move |source| {
                    ::serde_json::json!({
                        "Effect": POLICY_EFFECT_ALLOW,
                        // A resource policy names WHO is permitted; for a resource
                        // permission that is the source resource the caller granted.
                        "Principal": { "AWS": source },
                        "Action": action,
                        "Resource": resource_arn,
                    })
                })
            })
            .collect();
        if statements.is_empty() {
            return None;
        }
        Some(
            ::serde_json::json!({
                "Version": POLICY_VERSION,
                "Statement": statements,
            })
            .to_string(),
        )
    }

    /// File an operation record against `application` and return its `OperationId`.
    ///
    /// The record starts `INPROGRESS` with its success due [`TRANSITION_SPAN`] later;
    /// [`StoredOperation::settle`] materialises the transition once the clock reaches
    /// it. The id is a fresh UUID, which is what the model's `OperationId` pattern
    /// describes.
    pub fn record_operation(
        &mut self,
        op_type: &str,
        application: &StoredApplication,
        properties: HashMap<String, String>,
        now: SystemTime,
    ) -> String {
        self.record_operation_outcome(op_type, application, properties, now, None)
    }

    /// File an operation record whose work has an outcome of its own: `refusal` is what
    /// the work ran into, so the record reaches `ERROR` carrying it instead of `SUCCESS`.
    ///
    /// The outcome is known when the record is filed and reached when the clock brings
    /// the record to its terminal status, exactly as a success is: the work is
    /// asynchronous either way, so a caller polling straight after the call still observes
    /// it `INPROGRESS`. `None` is the plain success [`Self::record_operation`] files.
    pub fn record_operation_outcome(
        &mut self,
        op_type: &str,
        application: &StoredApplication,
        properties: HashMap<String, String>,
        now: SystemTime,
        refusal: Option<String>,
    ) -> String {
        let id = new_operation_id();
        self.operations.insert(
            id.clone(),
            StoredOperation {
                id: id.clone(),
                op_type: op_type.to_string(),
                status: OPERATION_STATUS_INPROGRESS.to_string(),
                // `StatusMessage` explains a non-success status; an operation that is
                // still running has nothing to explain.
                status_message: None,
                properties,
                resource_type: APPLICATION_RESOURCE_TYPE.to_string(),
                resource_id: application.id.clone(),
                resource_arn: application.arn.clone(),
                // The registration the call acted on, by the identity that is minted per
                // registration rather than by the id it reused
                // ([`StoredOperation::registration`]).
                registration: application.app_registry_arn.clone(),
                start_time: now,
                // An operation that has not finished has no end time yet.
                end_time: None,
                last_updated_time: now,
                terminal_due_at: now + TRANSITION_SPAN,
                // What the work this record tracks comes to: the refusal it ran into, or
                // the success it reaches when it ran into none.
                terminal_status: if refusal.is_some() {
                    OPERATION_STATUS_ERROR.to_string()
                } else {
                    OPERATION_STATUS_SUCCESS.to_string()
                },
                terminal_status_message: refusal,
                // The resources an operation reaches beyond the one the call named are
                // recorded as the work on each of them is asked for and answered
                // ([`Self::record_operation_target`]); a record starts with none.
                targets: Vec::new(),
            },
        );
        evict_oldest(&mut self.operations, StoredOperation::eviction_rank);
        id
    }

    /// Record that the operation `operation_id` acted on `target`, a resource other than
    /// the one the call named, so the operation reports reaching it
    /// ([`StoredOperation::events`]).
    ///
    /// Called once the service that owns that resource has answered, which is what lets
    /// the milestone carry the outcome rather than a status nothing produced. A record
    /// this Region no longer holds — one a later call's eviction already dropped — gets
    /// nothing, which is why no record is minted here.
    pub fn record_operation_target(&mut self, operation_id: &str, target: OperationTarget) {
        if let Some(record) = self.operations.get_mut(operation_id) {
            record.targets.push(target);
        }
    }

    /// The operation records filed against `application_id`, in the order a listing
    /// reports and pages them: by their own `OperationId`, which is the order
    /// [`Self::operations`] is keyed in, so the keys a page is taken over are already
    /// ascending and a `NextToken` resumes where the previous page stopped.
    ///
    /// A record names the REGISTRATION the call that filed it acted on
    /// ([`StoredOperation::registration`]), so the records of an application are the ones
    /// filed against the registration this id currently names — and an id this Region
    /// holds no application under has no records, the way it holds no tags and no
    /// permissions ([`Self::forget_resource`]). Every such record names that application's
    /// own id and ARN, which is what the listing reports as the `ResourceId` and
    /// `ResourceArn` of each of them.
    ///
    /// Joining on the registration rather than on the id is what keeps a registration from
    /// inheriting a dead one's history: an `ApplicationId` is free again the instant a
    /// deregistration completes, so a later registration can be made under the same id —
    /// and under the same ARN, these being a pure function of the ids they embed — but it
    /// is a NEW application that carries nothing of the old one, so the calls made on that
    /// one are no part of what has been done to this one. Those records are not destroyed:
    /// they are the history of calls already made, which `GetOperation` resolves by id,
    /// and [`MAX_RETAINED_OPERATIONS`] is what bounds them.
    ///
    /// Call [`Self::settle`] first, so a record whose terminal status the clock already
    /// owes is reported in it.
    pub fn operations_for(&self, application_id: &str) -> Vec<&StoredOperation> {
        let Some(application) = self.applications.get(application_id) else {
            return Vec::new();
        };
        self.operations
            .values()
            .filter(|record| record.registration == application.app_registry_arn)
            .collect()
    }

    /// File a configuration check operation for `check_id` against `application_id` and
    /// return the record, so the caller reports the SAME thing a later
    /// `GetConfigurationCheckOperation` reads.
    ///
    /// The record starts `INPROGRESS` with its terminal status due [`TRANSITION_SPAN`]
    /// later — a configuration check is asynchronous, so a caller acting straight after
    /// the start call still observes it running — and
    /// [`StoredConfigurationCheckOperation::settle`] materialises that transition once
    /// the clock reaches it. The id is a fresh UUID, which is what the model's
    /// `OperationId` pattern describes.
    pub fn record_configuration_check(
        &mut self,
        application_id: &str,
        check_id: &str,
        now: SystemTime,
    ) -> StoredConfigurationCheckOperation {
        let record = StoredConfigurationCheckOperation {
            id: new_operation_id(),
            application_id: application_id.to_string(),
            check_id: check_id.to_string(),
            status: OPERATION_STATUS_INPROGRESS.to_string(),
            // `StatusMessage` explains a non-success status; a check that is still
            // running has nothing to explain.
            status_message: None,
            start_time: now,
            // A check that has not finished has no end time yet.
            end_time: None,
            terminal_due_at: now + TRANSITION_SPAN,
            // The results a run produces are written by the run, when it reaches its
            // terminal status; see [`StoredConfigurationCheckOperation::settle`].
            sub_check_results: Vec::new(),
        };
        self.configuration_check_operations
            .insert(record.id.clone(), record.clone());
        evict_oldest(
            &mut self.configuration_check_operations,
            StoredConfigurationCheckOperation::eviction_rank,
        );
        record
    }

    /// The sub-check result `id` names, wherever in this Region it was produced.
    ///
    /// A sub-check result is identified by its own id alone — `ListSubCheckRuleResults`
    /// takes no operation id beside it — so the lookup spans this Region's configuration
    /// check operations and stops at the run that produced it. It is scoped to ONE
    /// Region like every other lookup here: a result produced at another regional
    /// endpoint does not resolve through this one.
    pub fn sub_check_result(&self, id: &str) -> Option<&StoredSubCheckResult> {
        self.configuration_check_operations
            .values()
            .find_map(|check| check.sub_check_result(id))
    }

    /// The configuration check operation for `check_id` that is still running against
    /// `application_id`, if one is.
    ///
    /// Call [`Self::settle`] first, so a check whose terminal status the clock already
    /// owes does not read as running.
    pub fn configuration_check_in_progress(
        &self,
        application_id: &str,
        check_id: &str,
    ) -> Option<&StoredConfigurationCheckOperation> {
        self.configuration_check_operations.values().find(|record| {
            record.application_id == application_id
                && record.check_id == check_id
                && record.status == OPERATION_STATUS_INPROGRESS
        })
    }

    /// The configuration check operations filed against `application_id`, in the order
    /// a listing reports and pages them: by their own `OperationId`, which is the order
    /// [`Self::configuration_check_operations`] is keyed in, so the keys a page is taken
    /// over are already ascending and a `NextToken` resumes where the previous page
    /// stopped.
    ///
    /// `latest_per_check` is the `LATEST_PER_CHECK` listing mode: only the most recently
    /// STARTED record of each `ConfigurationCheckType` survives, so the listing holds one
    /// row per check the application has ever had run rather than every run of it. Two
    /// records started in the same instant are ordered by their ids, so the choice is
    /// deterministic. `false` is `ALL_OPERATIONS`: every record, nothing collapsed.
    ///
    /// Call [`Self::settle`] first, so a record whose terminal status the clock already
    /// owes is reported in it.
    pub fn configuration_check_operations_for(
        &self,
        application_id: &str,
        latest_per_check: bool,
    ) -> Vec<&StoredConfigurationCheckOperation> {
        let mine = self
            .configuration_check_operations
            .values()
            .filter(|record| record.application_id == application_id);
        if !latest_per_check {
            return mine.collect();
        }
        let mut latest: BTreeMap<&str, &StoredConfigurationCheckOperation> = BTreeMap::new();
        for record in mine {
            latest
                .entry(record.check_id.as_str())
                .and_modify(|held| {
                    if (record.start_time, record.id.as_str())
                        > (held.start_time, held.id.as_str())
                    {
                        *held = record;
                    }
                })
                .or_insert(record);
        }
        // Back into the order the whole listing is paged in: by operation id.
        let mut kept: Vec<&StoredConfigurationCheckOperation> =
            latest.into_values().collect();
        kept.sort_by(|left, right| left.id.cmp(&right.id));
        kept
    }

    /// Drop every configuration check operation filed against `application_id`, and with
    /// it every result those runs produced — used when a deregistration completes, since
    /// a check run reports the configuration of an application that no longer exists and
    /// nothing else would ever reclaim it.
    pub fn forget_configuration_checks_for(&mut self, application_id: &str) {
        self.configuration_check_operations
            .retain(|_, record| record.application_id != application_id);
    }
}

/// Keep `records` within [`MAX_RETAINED_OPERATIONS`], dropping the oldest record that
/// has already reached a terminal status first so a caller polling a fresh record can
/// still resolve it.
///
/// `rank` orders the candidates for eviction: the record with the smallest rank goes
/// first, so a rank of `(still running, start time)` evicts the oldest finished record
/// before any running one.
fn evict_oldest<T, F, K>(records: &mut BTreeMap<String, T>, rank: F)
where
    F: Fn(&T) -> K,
    K: Ord,
{
    while records.len() > MAX_RETAINED_OPERATIONS {
        let victim = records
            .iter()
            .min_by_key(|(_, record)| rank(record))
            .map(|(id, _)| id.clone());
        match victim {
            Some(id) => {
                records.remove(&id);
            }
            None => break,
        }
    }
}

/// Per-service state for com-amazonaws-ssmsap-ssmsap. A handler reaches it through [`service_state`].
///
/// Systems Manager for SAP is a REGIONAL service, so the store holds one
/// [`RegionState`] per Region rather than one flat set of applications: every handler
/// resolves the Region its request was signed for ([`request_region`]) and then reaches
/// that Region's partition alone. The partitions are a `BTreeMap` so the store has one
/// stable order whatever order Regions were first written in.
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmsapSsmsapState {
    /// One partition per Region that holds something. Bounded by [`MAX_REGIONS`];
    /// written only through [`Self::region_entry`], so a READ never mints one.
    regions: BTreeMap<String, RegionState>,
}

impl ComAmazonawsSsmsapSsmsapState {
    /// Materialise every change the clock already owes, in every Region.
    ///
    /// Every handler calls this first. It is not restricted to the request's Region
    /// because the change was due at its own instant everywhere, whether or not
    /// anybody called at that endpoint.
    pub fn settle(&mut self, now: SystemTime) {
        for region in self.regions.values_mut() {
            region.settle(now);
        }
        // Settling a Region can empty it (the last deregistration it held completed), and
        // a partition that holds nothing is not one this store has to keep. This is the ONE
        // place the store reclaims a partition: it runs for every Region on every request,
        // so there is nothing a per-Region reclamation could add, and dropping an empty
        // partition changes nothing a caller can observe — an absent partition and an empty
        // one read the same ([`Self::region`]), while [`MAX_REGIONS`] then bounds only the
        // Regions that actually hold something.
        self.regions.retain(|_, region| !region.is_empty());
    }

    /// What a request signed for `region` may observe: that Region's partition, and
    /// nothing else.
    ///
    /// A Region nothing has ever been stored in reads as an EMPTY partition, so a read
    /// answers honestly for any Region a caller names WITHOUT minting a partition for
    /// it — which is what keeps [`MAX_REGIONS`] a bound on what was actually written.
    pub fn region(&self, region: &str) -> &RegionState {
        static EMPTY: ::std::sync::OnceLock<RegionState> = ::std::sync::OnceLock::new();
        self.regions
            .get(region)
            .unwrap_or_else(|| EMPTY.get_or_init(RegionState::default))
    }

    /// The partition of `region` for WRITING, created when this is the first thing
    /// stored there. A caller that has already found a resource in `region` through
    /// [`Self::region`] creates nothing by reaching for it.
    ///
    /// [`MAX_REGIONS`] bounds how many partitions this store keeps, and the check that
    /// holds that bound sits on the ONE call that can store the first thing in a Region:
    /// `RegisterApplication` refuses with its modelled `ConflictException` when the store
    /// already holds that many Regions and this one is new ([`Self::holds_region`] /
    /// [`Self::region_count`]). Every OTHER write first RESOLVES a resource of the Region
    /// through [`Self::region`] — a registered application, or an ARN
    /// [`RegionState::holds_resource`] answers for — and a Region that answers for one
    /// already has a partition, so none of them can create one. What a request names is
    /// therefore never what decides how many partitions exist, and a partition left
    /// holding nothing is dropped again by the next request ([`Self::settle`]), so only
    /// Regions that hold something count against the bound.
    pub fn region_entry(&mut self, region: &str) -> &mut RegionState {
        self.regions.entry(region.to_string()).or_default()
    }

    /// Whether the store already holds a partition for `region`.
    pub fn holds_region(&self, region: &str) -> bool {
        self.regions.contains_key(region)
    }

    /// How many Region partitions the store holds, which [`MAX_REGIONS`] bounds.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }
}

/// A fresh `OperationId`: a v4 UUID, the form the model's `OperationId` pattern
/// (`^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$`) describes.
pub fn new_operation_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// A fresh `SubCheckResultId` / `RuleResultId`: the same v4 UUID form BOTH patterns
/// describe, minted by the one generator this crate decides an id's form in
/// ([`new_operation_id`]), so the two cannot drift apart.
pub fn new_result_id() -> String {
    new_operation_id()
}

/// The ARN of the application `application_id` in `region`:
/// `arn:aws:ssm-sap:<region>:<account>:application/<ApplicationId>`.
///
/// The account segment comes from the core's single source of truth, never a literal.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_RESOURCE_TYPE_APPLICATION}/{application_id}"),
    )
}

/// The ARN namespace of an AWS Service Catalog AppRegistry application — an
/// AppRegistry application is a Service Catalog resource, which is why the model's
/// `AppRegistryArn` pattern (`^arn:aws:servicecatalog:[a-z0-9:\/-]+$`) admits only
/// ARNs of this service.
pub const APP_REGISTRY_ARN_SERVICE: &str = "servicecatalog";

/// The ARN of the AppRegistry application `app_registry_application_id` in `region`:
/// `arn:aws:servicecatalog:<region>:<account>:/applications/<AppRegistryApplicationId>`.
///
/// The account segment comes from the core's single source of truth, never a literal,
/// exactly as in [`application_arn`].
pub fn app_registry_arn(region: &str, app_registry_application_id: &str) -> String {
    ::engine::emulator_arn(
        APP_REGISTRY_ARN_SERVICE,
        region,
        &format!("/applications/{app_registry_application_id}"),
    )
}

/// The ARN namespace of an Amazon EC2 instance — an instance is EC2's resource, so its
/// ARN carries that service's namespace and not this one's.
pub const EC2_ARN_SERVICE: &str = "ec2";

/// The ARN of the Amazon EC2 instance `instance_id` in `region`:
/// `arn:aws:ec2:<region>:<account>:instance/<Ec2InstanceId>`, which is the form
/// `Resource$ResourceArn`'s own documentation gives for an instance
/// (`smithy_query(query="doc", shape="com.amazonaws.ssmsap#Resource",
/// member="ResourceArn")`).
///
/// The account segment comes from the core's single source of truth, never a literal,
/// exactly as in [`application_arn`].
pub fn ec2_instance_arn(region: &str, instance_id: &str) -> String {
    ::engine::emulator_arn(EC2_ARN_SERVICE, region, &format!("instance/{instance_id}"))
}

/// A fresh AppRegistry application id: the id of the AppRegistry application the
/// service records a registration in, which no request carries, so it is assigned
/// rather than echoed.
///
/// A v4 UUID written without its separators, which makes the id — and therefore the
/// ARN built from it ([`app_registry_arn`]) — lowercase alphanumeric, the only form
/// the modelled `AppRegistryArn` pattern (`^arn:aws:servicecatalog:[a-z0-9:\/-]+$`)
/// admits, and unique per registration.
pub fn new_app_registry_application_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// The id of the component a registration declared with `sid` and `component_type`.
///
/// A component is identified by the System ID and the type it was declared with, so two
/// entries describing the same component are one component — and the HANA component a
/// database belongs to is named the same way as one the registration listed.
pub fn component_id(sid: &str, component_type: &str) -> String {
    format!("{sid}-{component_type}")
}

/// The components of an application its registration DESCRIBES: the ones its
/// `components_info` declared, followed by the SAP HANA component of the SAP system `sid`
/// names when nothing declared that one already.
///
/// This is the landscape the service's discovery of the application has to find, not what
/// the application already has: a described component becomes a component of the
/// application only once that discovery has completed
/// ([`StoredApplication::components`]). A registration describes a component in EITHER of
/// two places:
///
/// * each `ComponentsInfo` entry describes one component: the type it runs, the System ID
///   of the SAP system it belongs to, and the EC2 instance it runs on.
/// * naming a `Sid` describes the SAP system of the application itself, whose SAP HANA
///   component is identified by that System ID and that type ([`component_id`]) — the
///   component the application's SAP HANA databases belong to
///   ([`StoredApplication::databases`]). It runs on the instance the registration named,
///   there being exactly one (`InstanceList` is `@length(min: 1, max: 1)`), so a
///   registration that named no System ID or no instance describes no such component.
///
/// A component is identified by its id, so a declaration of the component the `Sid`
/// already describes is that ONE component, declared: the declaration comes first and
/// keeps the EC2 instance it was declared on. The result therefore holds at most
/// [`MAX_COMPONENTS_PER_APPLICATION`] components, and every `ComponentId` a response
/// reports for this application resolves to one of them.
pub fn registered_components(
    sid: Option<&str>,
    instances: &[String],
    components_info: Vec<ComponentInfo>,
) -> Vec<StoredComponent> {
    let mut components: Vec<StoredComponent> = Vec::new();
    for component in components_info {
        let id = component_id(&component.sid, &component.component_type);
        if components.iter().any(|stored| stored.id == id) {
            continue;
        }
        components.push(StoredComponent {
            id,
            component_type: component.component_type,
            sid: component.sid,
            ec2_instance_id: component.ec2_instance_id,
        });
    }
    if let (Some(sid), Some(instance)) = (sid, instances.first()) {
        let id = component_id(sid, COMPONENT_TYPE_HANA);
        if !components.iter().any(|stored| stored.id == id) {
            components.push(StoredComponent {
                id,
                component_type: COMPONENT_TYPE_HANA.to_string(),
                sid: sid.to_string(),
                ec2_instance_id: instance.clone(),
            });
        }
    }
    components
}

/// The ARN of the component `component_id` of the application `application_id` in
/// `region`: `arn:aws:ssm-sap:<region>:<account>:component/<ApplicationId>/<ComponentId>`.
///
/// A component is a sub-resource of its application, so its ARN names both ids, exactly
/// as a database's does ([`database_arn`]); the account segment comes from the core's
/// single source of truth, never a literal.
pub fn component_arn(region: &str, application_id: &str, component_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_RESOURCE_TYPE_COMPONENT}/{application_id}/{component_id}"),
    )
}

/// The `ComponentStatus` of a component of an application, given the `ApplicationStatus`
/// that application's running state follows from
/// ([`StoredApplication::established_status`]) — `None` where the service has established
/// none.
///
/// A component runs as part of its application: starting and stopping an application is
/// what starts and stops its components, so a component is up exactly while the
/// application is up and is in the same transition while the application is in one. Where
/// the service has not established what the application is running (its registration is
/// still under way, or its discovery never succeeded) no component status follows from it,
/// which is what the model's `UNDEFINED` states.
pub fn component_status_for(established_status: Option<&str>) -> &'static str {
    match established_status {
        Some(APPLICATION_STATUS_ACTIVATED) => COMPONENT_STATUS_RUNNING,
        Some(APPLICATION_STATUS_STARTING) => COMPONENT_STATUS_STARTING,
        Some(APPLICATION_STATUS_STOPPING) => COMPONENT_STATUS_STOPPING,
        Some(APPLICATION_STATUS_STOPPED) => COMPONENT_STATUS_STOPPED,
        _ => COMPONENT_STATUS_UNDEFINED,
    }
}

/// The ARN of the database `database_id` of the application `application_id` in
/// `region`: `arn:aws:ssm-sap:<region>:<account>:database/<ApplicationId>/<DatabaseId>`.
///
/// A database is a sub-resource of its application, so its ARN names both ids; the
/// account segment comes from the core's single source of truth, never a literal.
pub fn database_arn(region: &str, application_id: &str, database_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_RESOURCE_TYPE_DATABASE}/{application_id}/{database_id}"),
    )
}

/// The KIND of resource an ARN of THIS service names: the resource type of its resource
/// field, which is the word the ARN was minted with ([`ARN_RESOURCE_TYPE_APPLICATION`],
/// [`ARN_RESOURCE_TYPE_COMPONENT`], [`ARN_RESOURCE_TYPE_DATABASE`]).
///
/// `None` when the ARN is not this service's — another service's resource, or a value
/// that is not a well-formed resource ARN at all — because the kind of such a resource is
/// that service's to decide and is not knowable here.
///
/// The ARN itself is read through the ONE parse the services share
/// (`service_common::arn`), so the fields and the `<type>/<id>` and `<type>:<id>` forms of
/// a resource are read here exactly as every other service reads them.
pub fn ssm_sap_resource_type(arn: &str) -> Option<&str> {
    ::service_common::arn::parse_of(arn, ARN_SERVICE).map(|parsed| parsed.resource_type())
}

/// Whether `arn` names a SAP HANA DATABASE, which is what the members that carry one are
/// documented to name — "The Amazon Resource Name of the SAP HANA database"
/// (`smithy_query(query="doc", shape="com.amazonaws.ssmsap#RegisterApplicationInput",
/// member="DatabaseArn")`, the same prose `UpdateApplicationSettings$DatabaseArn`
/// carries).
///
/// An ARN of this service names the resource its own resource type and ids identify
/// ([`ssm_sap_resource_type`]), so one that names a registered application or a component
/// names no database, whatever the request that carried it says it is: that resource
/// belongs to the registration whose ids its ARN embeds, and what is held on it — its
/// tags, the permissions granted on it — is held on THAT resource and goes when it goes
/// ([`RegionState::forget_resource`]). An ARN of any other service names a resource
/// outside this store, whose kind this service cannot know and holds nothing of, so a
/// registration naming one names its database under exactly that ARN.
pub fn arn_names_a_database(arn: &str) -> bool {
    ssm_sap_resource_type(arn)
        .is_none_or(|resource_type| resource_type == ARN_RESOURCE_TYPE_DATABASE)
}

/// The Region an ARN carries: the `region` field of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, empty for an ARN that has
/// none and for a value that is no resource ARN at all.
///
/// A partition holds the ARNs of ONE Region ([`RegionState`]), so this is how a record
/// that holds only its own ARN — the one [`RegionState::settle`] is removing, say —
/// recovers the Region its derived resources' ARNs are minted in
/// ([`StoredApplication::databases`]) without a handler passing it in.
///
/// Read through the ONE parse the services share (`service_common::arn`), the same one
/// [`ssm_sap_resource_type`] reads a resource's kind off, so this file holds no second
/// reader of the ARN grammar.
pub fn arn_region(arn: &str) -> &str {
    ::service_common::arn::parse(arn)
        .map(|parsed| parsed.region)
        .unwrap_or_default()
}

/// The `DatabaseId` of the SAP HANA database named `database_name`.
///
/// A database is identified by its name, so its id is a CANONICAL function of that name —
/// every name that names the same database yields the one id, which is the identity every
/// read of a database resolves by ([`StoredApplication::databases`], [`same_credential`],
/// [`database_type_for`], [`RegionState::resolve_database`]):
///
/// * every character the model's `DatabaseId` pattern (`[\w\d]+$`) does not admit is
///   replaced by `_`, which that pattern does admit, so two names differing only in such
///   characters are one database;
/// * every ASCII letter is folded to upper case, because an SAP HANA database name is
///   case-insensitive and HANA holds an unquoted identifier in upper case, so two names
///   differing only in case are one database too;
/// * the result is cut to the modelled `DatabaseId` length bound (1..300). A
///   `DatabaseName` is at most 100 characters, so only a name taken off a longer ARN
///   segment ([`database_name_from_arn`]) can reach that cut.
///
/// So a name of any form yields an id of the modelled form, and the same database always
/// yields the same id — the name a caller declared is kept as declared and reported in
/// `DatabaseName`, while the id, and the ARN minted from it, are that canonical form.
pub fn database_id_from_name(database_name: &str) -> String {
    database_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .take(MAX_DATABASE_ID_LENGTH)
        .collect()
}

/// Whether the SAP HANA database `database_id` is the database the reference `reference`
/// names, a `DatabaseId` a request carried.
///
/// A `DatabaseId` is the canonical form of a database's name
/// ([`database_id_from_name`]), and the case of a HANA database name is not part of its
/// identity, so a reference spelling the id in another case names that same database.
pub fn same_database_id(database_id: &str, reference: &str) -> bool {
    database_id.eq_ignore_ascii_case(reference)
}

/// Whether two `ApplicationCredential`s name the SAME credential of an application:
/// the one that reaches a given SAP HANA database with a given `CredentialType`.
///
/// The database is matched by its identity here ([`database_id_from_name`]), so the two
/// name the same credential exactly when they name the same database as every other
/// read of this service resolves it. `SecretId` is NOT part of the identity: it is the
/// secret that reaches the database, which is the thing an update replaces.
pub fn same_credential(one: &ApplicationCredential, other: &ApplicationCredential) -> bool {
    one.credential_type == other.credential_type
        && database_id_from_name(&one.database_name) == database_id_from_name(&other.database_name)
}

/// The credentials an application holds once the declarations `declared` are applied, in
/// the order the request named them, to the credentials `held`.
///
/// A declaration names a credential by the SAP HANA database it reaches and its
/// `CredentialType` ([`same_credential`]) and carries the `SecretId` that reaches it, so
/// one naming a credential the set already holds REPLACES it (the secret the latest
/// declaration names is the one that reaches that database from then on) and one naming a
/// credential it holds none of is ADDED after the ones it keeps. Two declarations in ONE
/// list naming the same credential are therefore that one credential, declared twice —
/// exactly as a repeated component declaration is one component
/// ([`registered_components`]) — so no list of declarations can leave an application
/// holding two credentials of one identity, and the same declarations leave the same
/// credentials whichever call carried them (`RegisterApplication$Credentials`,
/// `UpdateApplicationSettings$CredentialsToAddOrUpdate`).
pub fn credentials_with(
    held: &[ApplicationCredential],
    declared: &[ApplicationCredential],
) -> Vec<ApplicationCredential> {
    let mut credentials = held.to_vec();
    for named in declared {
        match credentials
            .iter_mut()
            .find(|holding| same_credential(holding, named))
        {
            Some(holding) => *holding = named.clone(),
            None => credentials.push(named.clone()),
        }
    }
    credentials
}

/// How an operation record names the credentials a call carried: the SAP HANA database
/// each one reaches and its `CredentialType`, in the order the call named them.
///
/// `ApplicationCredential$SecretId` is `@sensitive`, so the secret a credential names is
/// never written into a record a later `GetOperation` reads back; what identifies a
/// credential of an application is the database and the type anyway ([`same_credential`]).
/// Every call that carries credentials names them this one way, whichever member carried
/// them (`RegisterApplication$Credentials`,
/// `UpdateApplicationSettings$CredentialsToAddOrUpdate` and `$CredentialsToRemove`).
pub fn named_credentials(credentials: &[ApplicationCredential]) -> String {
    credentials
        .iter()
        .map(|credential| format!("{} ({})", credential.database_name, credential.credential_type))
        .collect::<Vec<String>>()
        .join(", ")
}

/// How an operation record names the components a call declared: each one's
/// [`component_id`] — the identity this service resolves a component by — and the Amazon
/// EC2 instance the declaration put it on, in the order the call named them.
pub fn named_components(components_info: &[ComponentInfo]) -> String {
    components_info
        .iter()
        .map(|component| {
            format!(
                "{} ({})",
                component_id(&component.sid, &component.component_type),
                component.ec2_instance_id
            )
        })
        .collect::<Vec<String>>()
        .join(", ")
}

/// The name the SAP HANA database ARN `database_arn` names its database by: the last
/// non-empty segment of the ARN, the `:`- and `/`-separated part that carries the name
/// of the resource an ARN names (a database ARN minted by [`database_arn`] ends with the
/// `DatabaseId`).
///
/// `None` when nothing past the ARN's separators is non-empty, so the ARN names no
/// database this service can identify and no database is derived from it.
pub fn database_name_from_arn(database_arn: &str) -> Option<&str> {
    database_arn
        .split([':', '/'])
        .rfind(|segment| !segment.is_empty())
}

/// The `DatabaseType` of the SAP HANA database named `database_name`.
///
/// The system database of a HANA system is the one named [`HANA_SYSTEM_DATABASE_NAME`];
/// every other database of the system is a tenant database. WHICH database a name names
/// is decided by the one identity the rest of this file resolves a database by
/// ([`database_id_from_name`], of whose canonical form the system database's name already
/// is), so a name this answers `SYSTEM` for is a name that derives the system database
/// and no second one.
pub fn database_type_for(database_name: &str) -> &'static str {
    if database_id_from_name(database_name) == HANA_SYSTEM_DATABASE_NAME {
        DATABASE_TYPE_SYSTEM
    } else {
        DATABASE_TYPE_TENANT
    }
}

/// The region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope (an unsigned call).
///
/// Deliberately a per-crate copy of a helper other service crates also hold: the
/// function that owns the parsing (`emulator_server::sigv4_credential_scope`) lives in
/// the vendored, read-only framework.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// What a handler says when [`service_state`] finds no store (this service mounted on
/// an engine that holds none): the emulator could not reach the place it keeps this
/// service's records, which is a fault of the emulator and nothing the caller sent.
///
/// Shared so every operation words that one fault the same way, whichever channel its
/// own model leaves it: [`store_unavailable`] for an operation that declares
/// `InternalServerException`, and `crate::scaffolded::Scaffolded::internal_failure` for
/// one that declares no server-fault shape at all.
pub const STORE_UNAVAILABLE_MESSAGE: &str = "the ssm-sap store is unavailable";

/// The modelled server error answered by a handler whose operation DECLARES
/// `InternalServerException` when [`service_state`] finds no store. The payload is
/// shared; each operation wraps it in its OWN generated error variant.
///
/// An operation whose model declares it for neither the operation nor the service — the
/// tagging calls — must not be widened with it: it answers the cross-service
/// `InternalFailure` through `crate::scaffolded::Scaffolded::internal_failure` instead.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some(STORE_UNAVAILABLE_MESSAGE.to_string()),
    }
}

/// This service's modelled `ValidationException` (HTTP 400) carrying `message`.
pub fn validation(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(message.into()),
    }
}

/// Refuse a member carrying a value outside the catalogue that member admits.
///
/// `Ok(())` when `value` is one of `catalogue`. This is the ONE place this service
/// refuses a value outside a catalogue, so every member checked this way is refused with
/// the same sentence: `member` worded as the API spells it, then the values the request
/// could have sent, then the one it did. A caller passes the catalogue the member is
/// checked against — the model's own enum values for an enum member
/// (`crate::ops::types::APPLICATION_TYPE_VALUES` and the other `*_VALUES` constants), or
/// the names one listing filters by ([`validate_filters`]) — and lifts the refusal into
/// its own modelled `ValidationException`.
///
/// An enum member is also walked by the boundary validator from the model, so for one of
/// those this cannot normally fire; it is here so no handler stores a value the API does
/// not define, whatever reaches it.
pub fn value_within_catalogue(
    member: &str,
    catalogue: &[&str],
    value: &str,
) -> Result<(), ValidationException> {
    if !catalogue.contains(&value) {
        return Err(validation(format!(
            "{member} must be one of {}, but was {value}",
            catalogue.join(", ")
        )));
    }
    Ok(())
}

/// A `RegisterApplication` input member the model leaves optional but the service
/// requires for SOME application types.
///
/// The model cannot express "required when `ApplicationType` is X"
/// (`smithy_query(query="operation", operation="RegisterApplication")` marks only
/// `ApplicationId`, `ApplicationType` and `Instances` `@required`), so the requirement
/// is conditional on the type the same request carries and belongs to the service.
/// Each variant knows the input member it names and the words a refusal names it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalMember {
    /// `RegisterApplication$Sid` — the application's System ID.
    Sid,
    /// `RegisterApplication$SapInstanceNumber` — the application's instance number.
    SapInstanceNumber,
    /// `RegisterApplication$Credentials` — the Secrets Manager credentials that reach
    /// the application's database. An EMPTY list specifies none, which is what the
    /// member's modelled `[]` default leaves behind when a request omits it.
    Credentials,
}

impl ConditionalMember {
    /// The input member this names, spelled as the model spells it.
    pub const fn member_name(self) -> &'static str {
        match self {
            Self::Sid => "Sid",
            Self::SapInstanceNumber => "SapInstanceNumber",
            Self::Credentials => "Credentials",
        }
    }

    /// How a refusal words the member — the service states the requirement in SAP's
    /// own vocabulary rather than with the wire member name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sid => "SID",
            Self::SapInstanceNumber => "instance number",
            Self::Credentials => "credentials",
        }
    }
}

/// What `RegisterApplication` requires of a registration of ONE `ApplicationType`,
/// beyond the modelled `@required` members.
///
/// The model cannot express a requirement conditional on another member's value
/// (`smithy_query(query="operation", operation="RegisterApplication")` marks only
/// `ApplicationId`, `ApplicationType` and `Instances` `@required`), so WHICH of the
/// optional members an application of a given type must carry — and which it must not —
/// belongs to the service. One row per value of the model's `ApplicationType` enum (see
/// [`crate::ops::types::APPLICATION_TYPE_VALUES`]), and the service states the whole rule
/// for the type in ONE sentence ([`Self::requirement`]) rather than per member, so a
/// client reads the same wording whichever member it got wrong.
#[derive(Debug, Clone, Copy)]
pub struct TypeMemberRule {
    /// The `ApplicationType` this row is about.
    pub application_type: &'static str,
    /// The members a registration of this type must CARRY. Empty when it requires
    /// nothing beyond the modelled members, so such a registration is accepted on those
    /// alone.
    pub required: &'static [ConditionalMember],
    /// The members a registration of this type must NOT carry: a member that names
    /// something only another type of application has, so carrying it describes an
    /// application this type cannot be.
    pub excluded: &'static [ConditionalMember],
    /// How the service states what a registration of this type has to specify — the
    /// phrase its refusal is built around ([`conditional_requirement_message`]), in SAP's
    /// own vocabulary rather than in wire member names. It names what the type requires,
    /// so it answers both a registration that left one of those out and one that
    /// described the application as another type instead.
    pub requirement: &'static str,
}

/// The member rules of every `ApplicationType` the model declares.
///
/// A HANA application IS its database: AWS Systems Manager for SAP cannot manage one
/// without the System ID and instance number that name the HANA instance and the
/// credentials that reach it — which is why every member of `ApplicationCredential`
/// (`DatabaseName`, `CredentialType`, `SecretId`) is documented in terms of "the SAP HANA
/// database".
///
/// An SAP_ABAP application is NOT its database: the SAP HANA database it runs on belongs
/// to the HANA application that manages it, so an ABAP registration names that database by
/// its ARN (`RegisterApplication$DatabaseArn`) and never by credentials of its own.
/// Credentials therefore describe a HANA registration, and one carried on an ABAP
/// registration is the registration's own account of what it is contradicting its
/// `ApplicationType` — which is why the refusal states what an ABAP registration has to
/// specify instead.
pub const CONDITIONALLY_REQUIRED_MEMBERS: &[TypeMemberRule] = &[
    TypeMemberRule {
        application_type: HANA_APPLICATION_TYPE,
        required: &[
            ConditionalMember::Sid,
            ConditionalMember::SapInstanceNumber,
            ConditionalMember::Credentials,
        ],
        excluded: &[],
        requirement: "SID, instance number, and credentials of HANA instance",
    },
    TypeMemberRule {
        application_type: ABAP_APPLICATION_TYPE,
        required: &[],
        excluded: &[ConditionalMember::Credentials],
        requirement: "The SID and database ARN of ASCS or Application Server",
    },
];

/// The member rule of `application_type`, and `None` for a type this table does not name,
/// which therefore requires and excludes nothing.
pub fn type_member_rule(application_type: &str) -> Option<&'static TypeMemberRule> {
    CONDITIONALLY_REQUIRED_MEMBERS
        .iter()
        .find(|rule| rule.application_type == application_type)
}

/// The members an application of `application_type` must carry beyond the modelled
/// `@required` ones. Empty when the type requires none.
pub fn conditionally_required_members(application_type: &str) -> &'static [ConditionalMember] {
    type_member_rule(application_type).map_or(&[], |rule| rule.required)
}

/// The members a registration of `application_type` must NOT carry. Empty when the type
/// excludes none.
pub fn excluded_members(application_type: &str) -> &'static [ConditionalMember] {
    type_member_rule(application_type).map_or(&[], |rule| rule.excluded)
}

/// Refuse a registration whose members do not describe an application of the
/// `ApplicationType` it carries ([`CONDITIONALLY_REQUIRED_MEMBERS`]).
///
/// `Ok(())` when the type's rule is met: every member it requires is carried and none it
/// excludes is. Otherwise the modelled `ValidationException` (HTTP 400) stating what a
/// registration of that type has to specify — one condition, so one sentence, whether the
/// request left a required member out or carried one that belongs to another type. The
/// caller must apply this BEFORE writing anything, so a refused registration leaves no
/// application behind.
pub fn conditional_requirements_met(
    application_type: &str,
    sid: Option<&str>,
    sap_instance_number: Option<&str>,
    credentials: &[ApplicationCredential],
) -> Result<(), ValidationException> {
    let Some(rule) = type_member_rule(application_type) else {
        return Ok(());
    };
    // A member is CARRIED when it specifies something: an empty list specifies nothing,
    // which is what the member's modelled `[]` default leaves behind for a request that
    // omits it — so it neither satisfies a requirement nor trips an exclusion.
    let carried = |member: ConditionalMember| match member {
        ConditionalMember::Sid => sid.is_some(),
        ConditionalMember::SapInstanceNumber => sap_instance_number.is_some(),
        ConditionalMember::Credentials => !credentials.is_empty(),
    };
    if rule.required.iter().copied().all(carried)
        && !rule.excluded.iter().copied().any(carried)
    {
        return Ok(());
    }
    Err(validation(conditional_requirement_message(application_type)))
}

/// How the service states what a registration of `application_type` has to specify
/// ([`TypeMemberRule::requirement`]).
///
/// One sentence for one condition, whichever call ran into it: a registration that omits
/// what its type requires, one that carries a member of another type, and an
/// `UpdateApplicationSettings` call that would leave an application without a member its
/// type requires all state the same rule — so a client reads one wording for it.
pub fn conditional_requirement_message(application_type: &str) -> String {
    let requirement = type_member_rule(application_type).map_or("", |rule| rule.requirement);
    format!(
        "{requirement} must be specified for SAP applications with type {application_type}."
    )
}

/// `labels`, joined as a sentence lists them: `A`, `A and B`, `A, B, and C`.
fn join_phrases(labels: &[&str]) -> String {
    match labels.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, [first])) => format!("{first} and {last}"),
        Some((last, head)) => format!("{}, and {last}", head.join(", ")),
    }
}

/// A setting of a registered application an `UpdateApplicationSettings` call changes.
///
/// One variant per settings member the model gives that operation
/// (`smithy_query(query="operation", operation="UpdateApplicationSettings")`:
/// `CredentialsToAddOrUpdate`/`CredentialsToRemove`, `Backint`, `DatabaseArn`), which is
/// what the call reports it is updating ([`settings_update_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsChange {
    /// The credentials that reach the application's SAP HANA databases, added, updated
    /// or removed.
    Credentials,
    /// The AWS Backint Agent for SAP HANA, whose configuration the call carries.
    /// `installed` is whether the application already holds one, so a first
    /// configuration is the agent's INSTALLATION — which is what the member is
    /// documented as — and a later one is a change to the configuration it holds.
    Backint { installed: bool },
    /// The application's SAP HANA connection, which the database the call names
    /// replaces.
    DatabaseConnection,
}

impl SettingsChange {
    /// How the service words the setting this names.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Credentials => "credentials",
            Self::Backint { installed: false } => "AWS Backint Agent for SAP HANA installation",
            Self::Backint { installed: true } => "AWS Backint Agent for SAP HANA configuration",
            Self::DatabaseConnection => "SAP HANA database connection",
        }
    }
}

/// The `Message` an `UpdateApplicationSettings` call answers with: the update of
/// `changes` it accepted for the application `application_id`.
///
/// The update is asynchronous — the call files the operation its `OperationIds` reports
/// and the settings fall due later ([`PendingSettings`]) — so what the message states is
/// that the request was taken, and it names the settings the call asked to change rather
/// than any value it carried, so nothing a credential's `@sensitive` `SecretId` holds is
/// ever worded into a response.
pub fn settings_update_message(application_id: &str, changes: &[SettingsChange]) -> String {
    let labels: Vec<&str> = changes.iter().map(|change| change.label()).collect();
    format!(
        "Your request to update the {} of application {application_id} has been submitted.",
        join_phrases(&labels)
    )
}

/// The message `UpdateApplicationSettings` reports when the request names NO setting to
/// change.
///
/// Every settings member the operation takes is optional
/// (`smithy_query(query="operation", operation="UpdateApplicationSettings")` marks only
/// `ApplicationId` `@required`), so a request carrying none of them asks for nothing.
///
/// A fixed diagnostic for a fixed condition — the request carries nothing to build it
/// from — so it is the service's own wording for this refusal verbatim, as the `ssm-sap`
/// application-control recording shows it: the condition is that NO update option was
/// named, stated without enumerating the members that could have carried one.
pub const MISSING_SETTINGS_MESSAGE: &str = "No update option was specified.";

/// The modelled `ValidationException` an operation that models no
/// `ResourceNotFoundException` is refused with when no registered application answers
/// the reference it named (`GetApplication`, `DeregisterApplication`).
///
/// A registration is scoped to an account and Region, so what the refusal states is
/// that THIS account holds no such application: it names the account the lookup ran in
/// and the reference the request carried. `GetApplication` models no
/// `ResourceNotFoundException` (`smithy_query(query="operation",
/// operation="GetApplication")`: `ValidationException` 400 and
/// `InternalServerException` 500 only), so an absent application is a 400 here rather
/// than a 404. The account comes from the core's single source of truth.
pub fn no_registered_application(reference: &str) -> ValidationException {
    validation(format!(
        "No registered application found for account: {}, and application: {reference}",
        ::engine::emulator_account_id()
    ))
}

/// The modelled `ValidationException` the operation-inspection listing is refused with
/// when the `ApplicationId` it requires names no application registered in this account
/// and Region (`ListOperations`).
///
/// The service words this condition PER OPERATION rather than once for the whole API: the
/// lookup an application-scoped listing of operation records runs states that the
/// application named does not exist, with the id the request carried quoted and nothing
/// of the environment the call reached in the text, while the application-lookup calls
/// state it as [`no_registered_application`] does. `ListOperations` models no
/// `ResourceNotFoundException` (`smithy_query(query="operation",
/// operation="ListOperations")`: `ValidationException` 400 and `InternalServerException`
/// 500 only), so the condition is the 400 it does model.
pub fn application_does_not_exist(application_id: &str) -> ValidationException {
    validation(format!("The application '{application_id}' does not exist"))
}

/// Why a `ComponentId` that names no component of the application it was asked for is
/// refused.
///
/// `GetComponent` models no `ResourceNotFoundException` (`smithy_query(query="operation",
/// operation="GetComponent")`: `ValidationException` 400, `UnauthorizedException` 401 and
/// `InternalServerException` 500 only), so a component id that resolves to nothing is the
/// `ValidationException` it does model rather than a 404 — and the refusal names both the
/// component asked for and the application the lookup ran in.
pub fn no_component_message(application_id: &str, component_id: &str) -> String {
    format!("No component with ID {component_id} exists for application {application_id}")
}

/// Why a reference that names no SAP HANA database of this account and Region is
/// refused.
///
/// `GetDatabase` models no `ResourceNotFoundException` (`smithy_query(query="operation",
/// operation="GetDatabase")`: `ValidationException` 400 and `InternalServerException` 500
/// only), so a reference that resolves to nothing is the `ValidationException` it does
/// model rather than a 404 — and the refusal is built from the request, naming the
/// `DatabaseArn` or `DatabaseId` the caller sent, whichever it resolved the database by.
pub fn no_database_message(reference: &str) -> String {
    format!("No database {reference} exists in this account and Region")
}

/// Why an `OperationId` that names no operation record of this account and Region is
/// refused.
///
/// The operations that take one (`GetOperation`, `ListOperationEvents`) model no 404, so
/// an id that resolves to nothing is the `ValidationException` they do model — and both
/// refuse it in the same words, so a client reads one wording for one condition whichever
/// of them it called. The template is AWS's, as the `ssm-sap` operation-lookup recording
/// shows it: fixed text followed by the `OperationId` the request carried verbatim, which
/// is the only variable part.
pub fn no_operation_message(operation_id: &str) -> String {
    format!("Operation Not Found: {operation_id}")
}

/// Why an `OperationId` that names no configuration check operation of this account and
/// Region is refused.
///
/// The operations that take one (`GetConfigurationCheckOperation`,
/// `ListSubCheckResults`) model no 404, so an id that resolves to nothing is the
/// `ValidationException` they do model — and both refuse it in the same words, naming
/// the id, so a caller can tell the two apart only by the call it made.
pub fn no_configuration_check_operation_message(operation_id: &str) -> String {
    format!(
        "No configuration check operation with ID {operation_id} exists in this account and Region"
    )
}

/// Why a `SubCheckResultId` that names no sub-check result of this account and Region is
/// refused, for the same reason as [`no_configuration_check_operation_message`]:
/// `ListSubCheckRuleResults` models no 404, so an id that resolves to nothing is its
/// `ValidationException`.
pub fn no_sub_check_result_message(sub_check_result_id: &str) -> String {
    format!(
        "No sub-check result with ID {sub_check_result_id} exists in this account and Region"
    )
}

/// Why a configuration check run ended in the model's failure status instead of
/// `SUCCESS`: the application it was started against stopped being registered before the
/// run finished, so the configuration its rules are about no longer exists.
pub fn configuration_check_subject_gone_message(application_id: &str) -> String {
    format!(
        "Application {application_id} is no longer registered, so the rules of this \
         configuration check were not evaluated"
    )
}

/// Why a run of a check this service catalogues no rules for
/// ([`CONFIGURATION_CHECK_CATALOGUE`]) ended in the model's failure status: there was
/// nothing to evaluate for it.
pub fn uncatalogued_configuration_check_message(check_id: &str) -> String {
    format!("No configuration check rules are defined for {check_id}")
}

/// This service's modelled `ResourceNotFoundException` (HTTP 404) carrying `message`.
pub fn not_found(message: impl Into<String>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(message.into()),
    }
}

/// The message a call reports when the application it names is not registered in this
/// account and Region.
///
/// Built from the request: the `ApplicationId` the caller sent is the only variable part,
/// so every handler that acts on or reads under the application a request names reports
/// the SAME template around it — which is what a client surfacing or matching on the
/// service's not-found wording reads. The template is AWS's, as the `ssm-sap`
/// application-control recordings show it, and it goes with the 404
/// `ResourceNotFoundException` those operations model. A call that models NO not-found
/// error answers the same condition with [`no_registered_application`] (400) instead.
pub fn application_not_found_message(reference: &str) -> String {
    format!("Application {reference} doesn't exist.")
}

/// The message a call that names a resource by ARN reports when this account and Region
/// hold no such resource.
///
/// ENTIRELY fixed text: the tagging routes state the condition and nothing of the request
/// that met it — not the ARN, not the kind of resource it named — so a client reads the
/// same sentence for every ARN that resolves to nothing. It is AWS's own wording for
/// those routes verbatim, as the `ssm-sap` tagging recording shows it, and it goes with
/// the wire error code those routes really carry
/// ([`crate::scaffolded::NOT_FOUND_CODE`], HTTP 404) rather than with the modelled
/// `ResourceNotFoundException` shape name. A call whose ARN has to name one KIND of
/// resource states that instead ([`invalid_database_arn`]), and a call that resolves an
/// application by its id states [`application_not_found_message`].
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The modelled `ValidationException` (HTTP 400) a resource-permission call is refused
/// with when its `ResourceArn` does not name a SAP HANA database of this account and
/// Region ([`RegionState::holds_database`]).
///
/// The permissions this service holds are DATABASE-scoped — a restore permission is
/// granted on the HANA database a backup would be restored into — so the ARN naming
/// anything else, including a registered application or a component of one, is an input
/// the caller has to correct rather than a resource that has gone: the refusal states
/// that the ARN is not a valid database ARN, with the ARN the request carried quoted as
/// the only variable part, and it is the 400 `ValidationException` every operation of
/// this service declares rather than the 404 the model also admits.
///
/// `member_name` is the input member the refusal attributes the value to, spelled as the
/// model spells it, for the call whose refusal tags it; `None` for the calls whose
/// refusal states the condition alone.
pub fn invalid_database_arn(member_name: Option<&str>, resource_arn: &str) -> ValidationException {
    let stated = format!("Database ARN '{resource_arn}' is not valid");
    validation(match member_name {
        Some(member) => format!("{member}: {stated}"),
        None => stated,
    })
}

/// The message `GetApplication` reports when the request carries NONE of the identifier
/// members that could name an application.
///
/// A fixed diagnostic for a fixed condition — the request carries nothing to build it
/// from — so it is AWS's own wording for this refusal verbatim, as the `ssm-sap`
/// application-lookup recording shows it. The identifier members this operation accepts
/// are still the three the model gives it (`smithy_query(query="operation",
/// operation="GetApplication")`); the message names the two an SAP application is
/// ordinarily referenced by.
pub const MISSING_APPLICATION_IDENTIFIER_MESSAGE: &str =
    "applicationId or applicationArn need to be specified";

/// The message `GetDatabase` reports when the request names no database at all.
///
/// Every member of the operation is optional (`smithy_query(query="operation",
/// operation="GetDatabase")`: `ApplicationId`, `ComponentId`, `DatabaseId`,
/// `DatabaseArn`), so which of them NAME a database to read is the service's rule: the
/// database's own ARN, or the application, component and database ids together. A
/// request carrying neither names nothing to read, and the diagnostic for that condition
/// is fixed — the request carries nothing to build it from — so it is the service's own
/// wording verbatim, as the `ssm-sap` application-lookup recording shows it, naming BOTH
/// alternatives rather than two of the four members.
pub const MISSING_DATABASE_IDENTIFIER_MESSAGE: &str =
    "Either database ARN or a combination of applicationId, componentId and databaseId \
     must be provided";

/// This service's modelled `ConflictException` (HTTP 409) carrying `message`.
pub fn conflict(message: impl Into<String>) -> ConflictException {
    ConflictException {
        message: Some(message.into()),
    }
}

/// The message `RegisterApplication` reports when the `ApplicationId` it was given is
/// already registered in this account and Region.
///
/// Built from the request: the `ApplicationId` the caller sent is the only variable part,
/// quoted, and the fixed text around it is AWS's own for this conflict verbatim, as the
/// `ssm-sap` registration recording shows it. `RegisterApplication` can raise
/// `ConflictException` for more than one reason, so the wording is what tells a duplicate
/// id apart from the others — which is why it has one construction site rather than being
/// spelled at the handler.
pub fn duplicate_application_id_message(application_id: &str) -> String {
    format!("Application id '{application_id}' already exists.")
}

/// A call that controls ONE registered application: the operations this service files
/// asynchronous work against an application with.
///
/// One variant per handler that has to refuse a request because work is already under way
/// on the application it names, so the condition AND its wording have a single
/// construction site ([`application_control_available`]) — the same convention
/// [`application_not_found_message`] states for the condition it reports, and the reason a
/// control path added later has something to call rather than a sixth copy to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationControl {
    /// `StartApplication`.
    Start,
    /// `StopApplication`.
    Stop,
    /// `StartApplicationRefresh`.
    Refresh,
    /// `UpdateApplicationSettings`.
    SettingsUpdate,
    /// `StartConfigurationChecks`.
    ConfigurationChecks,
}

impl ApplicationControl {
    /// How this call states what it could not do while the application is busy.
    const fn refusal(self) -> ControlRefusal {
        match self {
            Self::Start => ControlRefusal::Transition("started"),
            Self::Stop => ControlRefusal::Transition("stopped"),
            Self::Refresh => ControlRefusal::Transition("refreshed"),
            Self::SettingsUpdate => ControlRefusal::Outstanding("its settings cannot be updated"),
            Self::ConfigurationChecks => {
                ControlRefusal::Outstanding("configuration checks cannot be started")
            }
        }
    }
}

/// How a refused control call words the work already under way on the application.
enum ControlRefusal {
    /// A call that MOVES the application's own `Status` names the status it is in and the
    /// action it could not take, because that status is what the work in flight already
    /// reports to every reader.
    Transition(&'static str),
    /// A call that leaves the `Status` where it is — the settings an update leaves
    /// outstanding, the runs a check start would file — names the operation in flight
    /// instead, and the clause that operation blocks.
    Outstanding(&'static str),
}

/// Whether `control` can run against `application` now, or the modelled
/// `ConflictException` (HTTP 409) refusing it.
///
/// A change the clock still owes an application ([`StoredApplication::pending`]) is an
/// operation already under way on it, which is the condition `ConflictException` reports —
/// one rule, so one place decides it and one place words it, and every control call lifts
/// what comes back into its own modelled error variant. The caller applies this BEFORE
/// writing anything, so a refused call leaves the application exactly as it was.
///
/// STOPPING is the exception, and the registration is what it is an exception to: a stop
/// does not wait for the registration that is still running, because stopping an SAP
/// application is not a change to what that registration is establishing — so a caller
/// that registers an application and stops it straight away is served and gets the
/// operation it needs to track the stop, whatever the registration goes on to settle as.
pub fn application_control_available(
    application: &StoredApplication,
    control: ApplicationControl,
) -> Result<(), ConflictException> {
    if application.pending.is_none() {
        return Ok(());
    }
    if control == ApplicationControl::Stop
        && application.status == APPLICATION_STATUS_REGISTERING
    {
        return Ok(());
    }
    Err(conflict(match control.refusal() {
        ControlRefusal::Transition(action) => format!(
            "Application {} is {} and cannot be {action} until that completes",
            application.id, application.status
        ),
        ControlRefusal::Outstanding(clause) => format!(
            "Application {} has an operation in progress and {clause} until that \
             operation completes",
            application.id
        ),
    }))
}

/// The message a call that needs a WORKING application reports when the registration of
/// the application it names has not completed successfully
/// ([`StoredApplication::registered_successfully`]).
///
/// ENTIRELY fixed text: the condition is the application's own state and the refusal
/// states nothing of the request that met it, not even the id the request carried, so a
/// client reads the same sentence whichever application it named. It is the service's own
/// wording for this precondition verbatim, as the `ssm-sap` configuration-check recording
/// shows it, and it is what tells this condition apart from the other conflicts a control
/// call reports ([`application_control_available`]), since both carry the same error code.
pub const NOT_REGISTERED_SUCCESSFULLY_MESSAGE: &str =
    "The specified application has not been registered successfully";

/// The modelled `ConflictException` (HTTP 409) a call is refused with when the
/// registration of the application it names has not completed successfully.
///
/// The ONE construction site for that refusal, so every call that has this precondition
/// reports the same code and the same sentence ([`NOT_REGISTERED_SUCCESSFULLY_MESSAGE`]),
/// and it is decided by the application's registration outcome alone — never by what else
/// is in flight — so concurrent callers are all refused the same way whatever order they
/// arrive in.
pub fn not_registered_successfully() -> ConflictException {
    conflict(NOT_REGISTERED_SUCCESSFULLY_MESSAGE)
}

/// The message a call that moves an application's own status reports when the
/// registration of the application it names FAILED.
///
/// Built from the request: the `ApplicationId` the caller sent is the only variable part,
/// and the fixed text around it is the service's own for this condition verbatim, as the
/// `ssm-sap` application-control recording shows it. A failed registration is settled —
/// the application will not become usable without being registered again — so it is the
/// modelled `ValidationException` (HTTP 400) a caller has to act on rather than the
/// `ConflictException` that says to wait.
pub fn failed_registration_message(application_id: &str) -> String {
    format!("The registration of application {application_id} is failed.")
}

/// Whether a call that MOVES an application's own `Status` can act on `application`, or the
/// modelled `ValidationException` (HTTP 400) refusing it because that application's
/// registration FAILED ([`failed_registration_message`]).
///
/// A registration that failed discovered nothing, so there is no SAP system behind the
/// application for such a call to start or stop, and the `FAILED` status the registration
/// settled on is the application's own answer about it: a status move served here would
/// overwrite that answer with a running state the service never established, leaving the
/// two reads of one application contradicting each other. The refusal is settled — the
/// application does not become usable without being registered again — so it is the
/// `ValidationException` a caller has to act on rather than the `ConflictException` that
/// says to wait.
///
/// The ONE place that condition is decided, so every call that moves the application's own
/// `Status` refuses it with the same code and the same sentence,
/// and it is asked of the application's own registration outcome alone — never of what else
/// is in flight ([`application_control_available`] is the separate gate for that) — so
/// concurrent callers are all refused the same way whatever order they arrive in. A call
/// whose precondition is the STRONGER one, a registration that completed successfully
/// ([`not_registered_successfully`]), refuses a failed registration under that precondition
/// instead.
pub fn application_status_move_available(
    application: &StoredApplication,
) -> Result<(), ValidationException> {
    if application.registration_failed() {
        return Err(validation(failed_registration_message(&application.id)));
    }
    Ok(())
}

/// The message `RegisterApplication` reports when the account has already registered
/// every application it is allotted in this Region ([`APPLICATIONS_QUOTA`]).
///
/// `count` is what the account currently holds there, which at the boundary equals its
/// allotment: the refusal tracks the LIVE count rather than anything the request carried,
/// so deregistering one application lets the identical call through. The account comes
/// from the core's single source of truth, and the fixed text around the three values is
/// the service's own for this refusal verbatim, as the `ssm-sap` registration recording
/// shows it.
pub fn applications_quota_message(count: usize) -> String {
    format!(
        "Resource Limit Reached. Account {} is only allotted {APPLICATIONS_QUOTA} applications. Currently has {count} applications",
        ::engine::emulator_account_id()
    )
}

/// The `Name` values a `ListApplications` filter may carry: the members the model
/// gives `ApplicationSummary`, which is the shape the listing returns. Filter names
/// are case-sensitive, as `Filter$Name`'s documentation states.
pub const FILTER_NAMES: &[&str] = &["Id", "DiscoveryStatus", "Type", "Arn"];

/// The `Name` values a `ListConfigurationCheckOperations` filter may carry: the
/// string-valued members the model gives `ConfigurationCheckOperation`, which is the
/// shape that listing returns. Filter names are case-sensitive, as `Filter$Name`'s
/// documentation states. See [`StoredConfigurationCheckOperation::filter_attribute`].
pub const CONFIGURATION_CHECK_FILTER_NAMES: &[&str] =
    &["Id", "ApplicationId", "Status", "ConfigurationCheckId"];

/// The `Name` values a `ListOperations` filter may carry: the identifying and status
/// members the model gives `Operation`, which is the shape that listing returns. Filter
/// names are case-sensitive, as `Filter$Name`'s documentation states. See
/// [`StoredOperation::filter_attribute`].
pub const OPERATION_FILTER_NAMES: &[&str] = &[
    "Id",
    "Type",
    "Status",
    "ResourceType",
    "ResourceId",
    "ResourceArn",
];

/// The `Name` values a `ListOperationEvents` filter may carry, and the only operator it
/// accepts for them is [`FILTER_OPERATOR_EQUALS`] — both as that operation documents its
/// `Filters` member (`smithy_query(query="doc", shape="ListOperationEventsInput",
/// member="Filters")`). Filter names are case-sensitive, as `Filter$Name`'s documentation
/// states, so they are spelled here exactly as that member spells them. See
/// [`OperationEventRecord::filter_attribute`].
pub const OPERATION_EVENT_FILTER_NAMES: &[&str] = &["status", "resourceID", "resourceType"];

/// `FilterOperator` for exact equality; one of the model's `FilterOperator` values
/// (see [`crate::ops::types::FILTER_OPERATOR_VALUES`]).
pub const FILTER_OPERATOR_EQUALS: &str = "Equals";

/// `FilterOperator` for "at or after" the given value.
pub const FILTER_OPERATOR_GREATER_THAN_OR_EQUALS: &str = "GreaterThanOrEquals";

/// `FilterOperator` for "at or before" the given value.
pub const FILTER_OPERATOR_LESS_THAN_OR_EQUALS: &str = "LessThanOrEquals";

/// The longest `Filter$Value` a `ListApplications` request may carry.
///
/// The model gives `FilterValue` `@length(min: 1, max: 64)`
/// (`smithy_query(query="shape", shape="FilterValue")`), so the boundary validator
/// already refuses a longer value; [`filters_within_bounds`] re-states the bound HERE
/// so the work filtering does is bounded by something this crate can point at rather
/// than by a check that lives elsewhere.
pub const FILTER_VALUE_MAX_LENGTH: usize = 64;

/// The longest `Filter$Name` a `ListApplications` request may carry: the model gives
/// `FilterName` `@length(min: 1, max: 32)`.
pub const FILTER_NAME_MAX_LENGTH: usize = 32;

/// How many filters one `ListApplications` request may carry: the model gives
/// `FilterList` `@length(min: 1, max: 10)`.
pub const MAX_FILTERS: usize = 10;

/// The most pieces splitting one `Filter$Value` on `,` can yield, derived from
/// [`FILTER_VALUE_MAX_LENGTH`]: a value of `n` characters splits into at most `n + 1`
/// pieces. With [`MAX_FILTERS`] filters and at most [`APPLICATIONS_QUOTA`] applications,
/// the work [`filter_matches`] does for one request is bounded by a constant,
/// independent of the request body's size.
///
/// It is a CEILING on the split, never a truncation of the caller's list: a value the
/// model accepts cannot hold more pieces than this, so no value a caller sends is ever
/// dropped.
pub const MAX_FILTER_VALUES_PER_FILTER: usize = FILTER_VALUE_MAX_LENGTH + 1;

/// Refuse a filter list that exceeds the bounds the model declares.
///
/// `Ok(())` when every filter is within them. The boundary validator enforces the same
/// bounds from the model, so this cannot normally fire; it is here so the cost of
/// [`filter_matches`] over the whole store is bounded by a check in this crate.
///
/// A string `@length` is declared in CHARACTERS, so the two length bounds are measured
/// with [`str::chars`] — the unit the constraint is declared in, as
/// [`MAX_RULE_RESULT_METADATA_VALUE_LENGTH`] and [`MAX_DATABASE_ID_LENGTH`] are
/// elsewhere in this crate. Counting bytes instead would refuse a multi-byte value the
/// model admits and the boundary validator has already accepted, and would report that
/// byte count as though it were a character count.
pub fn filters_within_bounds(filters: &[Filter]) -> Result<(), ValidationException> {
    if filters.len() > MAX_FILTERS {
        return Err(validation(format!(
            "Filters holds at most {MAX_FILTERS} filters, but {} were given",
            filters.len()
        )));
    }
    for filter in filters {
        let name_length = filter.name.chars().count();
        if name_length > FILTER_NAME_MAX_LENGTH {
            return Err(validation(format!(
                "Filter Name is at most {FILTER_NAME_MAX_LENGTH} characters, but {name_length} were given"
            )));
        }
        let value_length = filter.value.chars().count();
        if value_length > FILTER_VALUE_MAX_LENGTH {
            return Err(validation(format!(
                "Filter Value is at most {FILTER_VALUE_MAX_LENGTH} characters, but {value_length} were given"
            )));
        }
    }
    Ok(())
}

/// Which `FilterOperator`s a filtered listing offers.
///
/// A listing either takes every operator the model's `FilterOperator` enum declares, or
/// documents ONE operator for its filters — so the catalogue a listing is checked
/// against, and the wording of the refusal, are the only way the shared rule differs
/// between them. See [`validate_filters`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperators<'a> {
    /// Every operator the model declares (see
    /// [`crate::ops::types::FILTER_OPERATOR_VALUES`]).
    Modelled,
    /// The one operator a listing documents for its filters, e.g.
    /// [`FILTER_OPERATOR_EQUALS`].
    Only(&'a str),
}

impl FilterOperators<'_> {
    /// Refuse an operator this listing does not offer, naming what it does offer — a
    /// listing offering the model's whole catalogue through the one place this service
    /// refuses a value outside a catalogue ([`value_within_catalogue`]).
    fn accepts(&self, operator: &str) -> Result<(), ValidationException> {
        match self {
            Self::Modelled => {
                value_within_catalogue("Filter Operator", FILTER_OPERATOR_VALUES, operator)?;
            }
            Self::Only(only) => {
                if operator != *only {
                    return Err(validation(format!(
                        "Filter Operator must be {only}, but was {operator}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Refuse a `Filters` member this listing cannot apply, whatever is being listed.
///
/// This is the validation every filtered listing of this service shares, so one rule
/// decides what makes a `Filter` usable: the bounds the model declares
/// ([`filters_within_bounds`]) first, then, for each filter in the order the request
/// carries them, an operator the listing offers (`operators`) and a `Name` in
/// `allowed_names` — the attributes that listing reports, which are what
/// [`filter_matches_attribute`] can then compare. A listing passes its own name
/// catalogue ([`FILTER_NAMES`], [`CONFIGURATION_CHECK_FILTER_NAMES`],
/// [`OPERATION_EVENT_FILTER_NAMES`]) and lifts the refusal into its own modelled
/// `ValidationException`. The name is checked by [`value_within_catalogue`], the one
/// place this service refuses a value outside a catalogue.
pub fn validate_filters(
    filters: &[Filter],
    operators: FilterOperators<'_>,
    allowed_names: &[&str],
) -> Result<(), ValidationException> {
    filters_within_bounds(filters)?;
    for filter in filters {
        operators.accepts(&filter.operator)?;
        value_within_catalogue("Filter Name", allowed_names, &filter.name)?;
    }
    Ok(())
}

/// Whether `application` satisfies one `ListApplications` filter.
///
/// Every filterable attribute is a string in the model, so `Equals` is exact
/// (case-sensitive) string equality and the two ordering operators compare the
/// attribute lexicographically. `Filter$Value`'s documentation says that "if you
/// specify multiple values for a filter, the values are joined with an OR, and the
/// request returns all results that match any of the specified values", so a
/// comma-separated `Value` matches when ANY of its values does. Several filters narrow
/// together.
///
/// The number of values compared is bounded: `FilterValue` is at most
/// [`FILTER_VALUE_MAX_LENGTH`] characters — enforced by the boundary validator from the
/// model AND re-stated by [`filters_within_bounds`] before any filter is applied — so
/// splitting it yields at most [`MAX_FILTER_VALUES_PER_FILTER`] pieces, whatever the
/// request body's size. The `take` makes that ceiling explicit at the only place the
/// caller's value is iterated; because it is the ceiling the length bound already
/// implies, it can never drop a value the caller sent.
pub fn filter_matches(application: &StoredApplication, filter: &Filter) -> bool {
    filter_matches_attribute(application.filter_attribute(&filter.name), filter)
}

/// Whether the attribute one filter names satisfies it, where `actual` is the value the
/// record reports for `filter.name` and `None` means the record has no such attribute
/// (which nothing matches).
///
/// This is the comparison every filtered listing of this service shares — see
/// [`filter_matches`] for `ListApplications` and
/// [`StoredConfigurationCheckOperation::filter_attribute`] for the configuration check
/// listing — so one rule decides what a `FilterOperator` means whatever is being listed.
pub fn filter_matches_attribute(actual: Option<&str>, filter: &Filter) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    filter
        .value
        .split(',')
        .take(MAX_FILTER_VALUES_PER_FILTER)
        .any(|wanted| match filter.operator.as_str() {
            FILTER_OPERATOR_EQUALS => actual == wanted,
            FILTER_OPERATOR_GREATER_THAN_OR_EQUALS => actual >= wanted,
            FILTER_OPERATOR_LESS_THAN_OR_EQUALS => actual <= wanted,
            // The caller's operator is checked against the model's own catalogue
            // before any filter is applied, so no other value reaches here.
            _ => false,
        })
}

/// One page of a listing: the items on it, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Why a listing refused its paging arguments.
#[derive(Debug)]
pub enum PageError {
    /// `NextToken` was not a token this listing, over the scope the request named,
    /// handed out.
    InvalidToken(String),
}

/// Which listing, taken over which scope, a continuation token belongs to.
///
/// A token is opaque to a client, but it still names a position in ONE listing taken over
/// ONE scope: its key is an item key of that listing, and it locates an item only among
/// the items that scope reports. A token handed out by another listing, whose keys mean
/// something else, or by the same listing over another scope, where the same key names a
/// position among other items, must therefore be refused rather than resumed from — so
/// the listing AND the values that narrow it are both part of what the token marks
/// ([`Listing::token_prefix`]).
///
/// What narrows a listing is the regional endpoint it was taken at, since every record of
/// this service belongs to one Region, plus the parent record or records the request
/// named. A `Filters` member is NOT part of it: filtering drops items from a listing
/// without changing the key order of the ones it keeps, so a key still names the same
/// position and a walk that keeps its filters still reports every matching item exactly
/// once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing<'a> {
    /// `ListApplications`, whose keys are `ApplicationId`s, over one Region.
    Applications { region: &'a str },
    /// `ListComponents`, whose keys are (`ApplicationId`, `ComponentId`) pairs
    /// ([`StoredComponent::page_key`]), over one Region and the one application the
    /// request named, if any.
    Components {
        region: &'a str,
        application_id: Option<&'a str>,
    },
    /// `ListDatabases`, whose keys are (`ApplicationId`, `DatabaseId`) pairs
    /// ([`ApplicationDatabase::page_key`]), over one Region and the application and
    /// component the request named, if any.
    Databases {
        region: &'a str,
        application_id: Option<&'a str>,
        component_id: Option<&'a str>,
    },
    /// `ListOperations`, whose keys are `OperationId`s, over the one application of one
    /// Region the request named.
    Operations {
        region: &'a str,
        application_id: &'a str,
    },
    /// `ListConfigurationCheckOperations`, whose keys are `OperationId`s, over the one
    /// application of one Region the request named.
    ConfigurationCheckOperations {
        region: &'a str,
        application_id: &'a str,
    },
    /// `ListConfigurationCheckDefinitions`, whose keys are `ConfigurationCheckType`s.
    /// The catalogue is the service's own, which neither a Region nor a record narrows,
    /// so this listing has one scope.
    ConfigurationCheckDefinitions,
    /// `ListOperationEvents`, whose keys are [`OperationEventRecord::page_key`]s, over
    /// the one operation record of one Region the request named.
    OperationEvents {
        region: &'a str,
        operation_id: &'a str,
    },
    /// `ListSubCheckResults`, whose keys are `SubCheckResultId`s, over the one
    /// configuration check operation of one Region the request named.
    SubCheckResults {
        region: &'a str,
        operation_id: &'a str,
    },
    /// `ListSubCheckRuleResults`, whose keys are `RuleResultId`s, over the one sub-check
    /// result of one Region the request named.
    SubCheckRuleResults {
        region: &'a str,
        sub_check_result_id: &'a str,
    },
}

/// How [`Listing::token_prefix`] writes a narrowing value a request did NOT carry — an
/// optional `ApplicationId` or `ComponentId` left out, which widens the listing to every
/// record of its Region. It is distinct from every present value, which is written with
/// its length first, because no id the model admits is empty.
const ABSENT_SCOPE_VALUE: &str = "-";

impl<'a> Listing<'a> {
    /// Marks a continuation token as this listing KIND's, and at which version it was
    /// issued.
    ///
    /// Long enough on its own that the hex rendering of any token always satisfies the
    /// model's `NextToken` pattern (`^.{16,2048}$`, so at least 16 characters).
    const fn kind_marker(self) -> &'static str {
        match self {
            Self::Applications { .. } => "ssmsap:applications:v1:",
            Self::Components { .. } => "ssmsap:components:v1:",
            Self::Databases { .. } => "ssmsap:databases:v1:",
            Self::Operations { .. } => "ssmsap:operations:v1:",
            Self::ConfigurationCheckOperations { .. } => "ssmsap:check-operations:v1:",
            Self::ConfigurationCheckDefinitions => "ssmsap:check-definitions:v1:",
            Self::OperationEvents { .. } => "ssmsap:operation-events:v1:",
            Self::SubCheckResults { .. } => "ssmsap:sub-check-results:v1:",
            Self::SubCheckRuleResults { .. } => "ssmsap:rule-results:v1:",
        }
    }

    /// The values that narrow what this listing reports, in a fixed order per kind: the
    /// Region the request was signed for and the parent record or records it named, each
    /// `None` when the request carried none.
    fn scope(self) -> Vec<Option<&'a str>> {
        match self {
            Self::Applications { region } => ::std::vec![Some(region)],
            Self::Components {
                region,
                application_id,
            } => ::std::vec![Some(region), application_id],
            Self::Databases {
                region,
                application_id,
                component_id,
            } => ::std::vec![Some(region), application_id, component_id],
            Self::Operations {
                region,
                application_id,
            }
            | Self::ConfigurationCheckOperations {
                region,
                application_id,
            } => ::std::vec![Some(region), Some(application_id)],
            Self::ConfigurationCheckDefinitions => Vec::new(),
            Self::OperationEvents {
                region,
                operation_id,
            }
            | Self::SubCheckResults {
                region,
                operation_id,
            } => ::std::vec![Some(region), Some(operation_id)],
            Self::SubCheckRuleResults {
                region,
                sub_check_result_id,
            } => ::std::vec![Some(region), Some(sub_check_result_id)],
        }
    }

    /// What every token of this listing, over this scope, begins with: the kind's own
    /// marker and then each of its narrowing values ([`Listing::scope`]).
    ///
    /// Each present value is written with its byte length first, so the sequence is
    /// self-delimiting and two different scopes cannot spell the same prefix however
    /// their values read — the Region comes from the request's own credential scope, so
    /// it is a value a caller chooses.
    fn token_prefix(self) -> String {
        let mut prefix = self.kind_marker().to_string();
        for value in self.scope() {
            match value {
                Some(value) => prefix.push_str(&format!("{}:{value}", value.len())),
                None => prefix.push_str(ABSENT_SCOPE_VALUE),
            }
        }
        prefix
    }
}

/// Joins the two parts of a composite listing key; see [`composite_page_key`].
///
/// U+0001 is BELOW every byte the modelled id patterns of this service admit: the lowest
/// character any of them allows is `-` (0x2D), in `ApplicationId` (`^[\w\d\.-]+$`) and
/// `ComponentId` (`^[\w\d-]+$`), and `DatabaseId` (`[\w\d]+$`) allows nothing below a
/// digit.
const PAGE_KEY_SEPARATOR: char = '\u{1}';

/// The key a listing ordered by a PAIR of ids is ordered and paged by: the id of the
/// resource the items belong to, then the id of the item within it.
///
/// The two are joined by [`PAGE_KEY_SEPARATOR`], a byte lower than any the modelled id
/// patterns admit, so ordering these keys as strings is the same as ordering by (outer
/// id, inner id) for every pair of ids the model accepts. A separator a pattern DOES
/// admit would not be: with one, an id another is a prefix of sorts AFTER that longer id
/// whenever the longer one continues with a lower character, and the listing would then
/// report the pairs in neither the order it documents nor the order its sibling listings
/// use. That equality is also what makes a `NextToken` resume at the item the previous
/// page stopped at.
pub fn composite_page_key(outer: &str, inner: &str) -> String {
    format!("{outer}{PAGE_KEY_SEPARATOR}{inner}")
}

/// Take one page out of an id-ordered listing.
///
/// `all` is every item that matches the request, paired with the key it is ordered by.
/// The page holds at most `max_results` items starting at `next_token`, and carries a
/// token only when further items remain — so a caller that follows tokens to exhaustion
/// sees every item exactly once and the paging terminates. `listing` is which listing,
/// over which scope, the token belongs to, so a token one listing issued is not resumed
/// from by another, nor by the same listing over another scope, where its key would name
/// a position among other items and silently drop every item sorting below it.
pub fn page<T>(
    all: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    listing: Listing<'_>,
) -> Result<Page<T>, PageError> {
    let start = match next_token {
        None => None,
        Some(token) => Some(
            decode_page_token(token, listing)
                .ok_or_else(|| PageError::InvalidToken(token.to_string()))?,
        ),
    };
    // `MaxResults` carries `@range(1..50)`, so the boundary validator has already
    // refused anything outside it; an absent `MaxResults` takes the largest page the
    // range allows, and the `clamp` keeps that true however the range is read.
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;

    let mut rest = all
        .into_iter()
        .skip_while(|(key, _)| start.as_deref().is_some_and(|from| key.as_str() < from));
    let items: Vec<T> = rest.by_ref().take(limit).map(|(_, item)| item).collect();
    // The token names the first item NOT on this page, so the next call resumes at it.
    let next_token = rest
        .next()
        .map(|(key, _)| encode_page_token(&key, listing));
    Ok(Page { items, next_token })
}

/// Render a paging refusal as this service's modelled `ValidationException`, naming
/// the member that was wrong so the caller can fix the request from the text alone.
pub fn page_validation_exception(error: &PageError) -> ValidationException {
    match error {
        PageError::InvalidToken(token) => {
            validation(format!("NextToken {token} is not a valid pagination token"))
        }
    }
}

/// Encode a listing key as the opaque `NextToken` that listing hands out.
///
/// The key is prefixed with the listing's own version marker and the scope the listing
/// was taken over before hex-encoding, so the token is opaque to a client (it is not the
/// resource id it was built from) AND a value this listing over this scope did not issue
/// is refused rather than resumed from. It stays a pure function of the key and the
/// scope, so the token is stable and carries no server-side cursor state.
fn encode_page_token(key: &str, listing: Listing<'_>) -> String {
    ::hex::encode(format!("{}{key}", listing.token_prefix()))
}

/// The inverse of [`encode_page_token`]; `None` for anything this listing, over this
/// scope, did not issue.
fn decode_page_token(token: &str, listing: Listing<'_>) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&listing.token_prefix())
        .map(str::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmsapSsmsapState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmsapSsmsapState>)
}
