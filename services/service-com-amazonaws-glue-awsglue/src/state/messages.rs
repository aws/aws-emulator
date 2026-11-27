// SPDX-License-Identifier: CC0-1.0

//! The wording of every refusal this service answers with, so one miss reads the same
//! wherever it is reported.

/// A crawler lookup (`GetCrawler`, `DeleteCrawler`, `StopCrawler`, and the listings and
/// definitions that resolve one the same way) for a name no crawler in this account has.
/// Built from the request's `Name`, which is stored and matched exactly as sent.
///
/// RECORDED template for the crawler-DEFINITION reads. The run surface words the very same
/// miss its own way ([`start_crawler_not_found`]), and so does the definition WRITE
/// ([`CRAWLER_NOT_FOUND_FOR_UPDATE`]), so none of the three may be borrowed for another.
pub fn crawler_not_found(name: &str) -> String {
    format!("Crawler entry with name {name} does not exist")
}

/// `UpdateCrawler` for a `Name` no crawler in this account has.
///
/// RECORDED constant, and the whole message: the definition WRITE states the miss over the
/// member the request addressed the crawler by and withholds the value, where the definition
/// READS name it ([`crawler_not_found`]) and the start of a crawl words it a third way
/// ([`start_crawler_not_found`]) — the sentence belongs to the operation that refuses and
/// not to the resource, so the requested name is NOT interpolated here.
pub const CRAWLER_NOT_FOUND_FOR_UPDATE: &str = "Crawler with provided name does not exist";

/// `StartCrawler` for a `Name` no crawler in this account has.
///
/// RECORDED template, and NOT [`crawler_not_found`]: the two are recorded side by side for
/// one and the same absent name — the crawler-definition read states "Crawler entry with
/// name ...", the start of a crawl states "Crawler with name ..." — so the sentence belongs
/// to the operation that refuses and not to the service. The only variable part is the
/// request's `Name`, echoed exactly as sent; the rest is fixed text.
///
/// Pinned for THIS operation, the one the corpus records. The sibling run call
/// (`StopCrawler`) keeps its own wording rather than being reworded on this operation's
/// evidence.
pub fn start_crawler_not_found(name: &str) -> String {
    format!("Crawler with name {name} does not exist")
}

/// The schedule-STATE toggles — `StartCrawlerSchedule` and `StopCrawlerSchedule` — for a
/// `CrawlerName` no crawler in this account has.
///
/// Recorded template, reproduced as the service words this error class and NOT shortened:
/// the sentence names the crawler the request addressed — the only variable part, read off
/// the request's `CrawlerName` — states that it does not exist, and then states the miss a
/// second time, because the component that answers this refusal composes its own sentence
/// with the generic one, exactly as the classifier read's miss
/// ([`classifier_not_found`]) does. A client matching on the refusal sees that doubled
/// wording, so dropping the second half would be a divergence rather than a tidy-up. This
/// is why these operations' miss is not [`crawler_not_found`], the sentence the
/// crawler-definition reads answer with.
///
/// Shared by BOTH toggles, which the corpus records answering one and the same sentence for
/// one and the same absent name: the pair decides only the schedule's `State`, so the miss
/// belongs to the schedule surface that answers it. `UpdateCrawlerSchedule` writes the
/// crawler's DEFINITION instead (it replaces or removes the cron expression), so it keeps
/// the definition reads' wording above; no recording pins any other sentence for it.
pub fn schedule_crawler_not_found(name: &str) -> String {
    format!("Crawler with name {name} does not exist not found.")
}

/// `UpdateCrawler` for a crawler whose crawl is still in progress ("if a crawler is
/// running, you must stop it using `StopCrawler` before updating it"). NOT recorded:
/// what is pinned is the behaviour — 400 with the operation's modelled
/// `CrawlerRunningException` — and, like the recorded crawler miss above, a sentence
/// carrying only the requested `Name`.
pub fn crawler_running(name: &str) -> String {
    format!("Crawler with name {name} is running")
}

/// `DeleteCrawler` for a crawler whose crawl is in progress ("removes a specified crawler
/// from the Glue Data Catalog, unless the crawler state is RUNNING").
///
/// RECORDED template, reproduced as the service words this error class and NOT tidied: the
/// fixed text that names the delete it refuses runs STRAIGHT into the crawler's `Name`, with
/// no separating space, before the fixed tail. That is the service's own wording, so
/// inserting the space a reader expects would be a divergence rather than a repair. The only
/// variable part is the request's `Name`, echoed exactly as sent.
///
/// NOT [`crawler_running`], which is the refusal a WRITE to the crawler's definition earns
/// and which no recording pins: each operation states the refusal it makes in its own words.
pub fn cannot_delete_running_crawler(name: &str) -> String {
    format!("Cannot delete crawler{name} while it's running")
}

/// `StartCrawler` for a crawler whose crawl is already in progress: "if the crawler is
/// already running, returns a `CrawlerRunningException`" (the operation's own
/// documentation). NOT recorded: what is pinned is the behaviour — 400 with the
/// operation's modelled `CrawlerRunningException` — and, like the crawler miss above, a
/// sentence carrying only the requested `Name`. Worded for the START it refuses rather
/// than reusing [`crawler_running`], which is the refusal a WRITE to the crawler's
/// definition earns.
pub fn crawler_already_started(name: &str) -> String {
    format!("Crawler with name {name} has already started")
}

/// `StopCrawler` for a crawler that has no crawl under way: "if the specified crawler is
/// running, stops the crawl", so a crawler sitting in the state with no run in it
/// ([`super::CRAWLER_STATE_READY`]) has no crawl for the call to stop.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `CrawlerNotRunningException` — and, like the crawler-running refusal above, a sentence
/// carrying only the requested `Name`, worded as this service words that family.
pub fn crawler_not_running(name: &str) -> String {
    format!("Crawler with name {name} is not running")
}

/// `StopCrawler` for a crawler whose crawl has ALREADY been asked to stop and has not
/// finished ([`super::CRAWLER_STATE_STOPPING`]): the stop it asks for is already under way,
/// which is the operation's own modelled `CrawlerStoppingException`.
///
/// NOT recorded, on the same grounds as [`crawler_not_running`].
pub fn crawler_stopping(name: &str) -> String {
    format!("Crawler with name {name} is stopping")
}

/// A request to suspend the schedule of a crawler that has NO schedule at all — one
/// carrying no cron expression to run ([`super::CrawlerEntry::has_schedule`]) — so there
/// is no scheduler for the call to stop. A crawler whose schedule is merely suspended is
/// SERVED, because the state that call writes is the state it is in.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `SchedulerNotRunningException`, the only class it declares for a crawler with no
/// schedule — and, like the crawler-running refusal above, a sentence carrying only the
/// requested `CrawlerName`.
pub fn scheduler_not_running(name: &str) -> String {
    format!("Scheduler for crawler with name {name} is not running")
}

/// A request to put the schedule of a crawler that carries NO cron expression back in
/// force ([`super::CrawlerEntry::has_schedule`]): there is no schedule to start.
///
/// RECORDED constant, and the whole message: the service states that no schedule was found
/// and echoes back neither the crawler the request addressed nor the state its schedule
/// would have been in — unlike the not-running refusal above, whose sentence names the
/// crawler. The refusal is the operation's modelled `NoScheduleException`.
pub const NO_SCHEDULE_FOUND: &str = "No schedule found.";

/// A CRAWLER-surface write whose `Schedule` is not the `cron(..)` expression the service
/// documents ([`super::is_cron_expression`]). NOT recorded: what is pinned is the
/// behaviour — 400 with the operation's modelled `InvalidInputException` — and a
/// sentence carrying only the requested expression plus the form it has to take.
/// Shared by every crawler-surface operation that writes that member (`CreateCrawler`,
/// `UpdateCrawler`, `UpdateCrawlerSchedule`, and the column statistics task settings,
/// which schedule a run the same way), so one refusal cannot drift from another's.
///
/// The TRIGGER surface words the same defect its own way
/// ([`trigger_schedule_invalid`]), which is why the two are not one string.
pub fn schedule_not_a_cron_expression(expression: &str) -> String {
    format!(
        "Schedule {expression} is not a cron expression of the form \
             cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)"
    )
}

/// `CreateCrawler` (and the `UpdateCrawler` that would replace a crawler's targets with
/// none) whose `CrawlerTargets` names no crawl target at all — no `S3Targets`,
/// `JdbcTargets`, `DynamoDBTargets`, or any other target list the shape carries
/// ([`super::has_crawl_target`]), whether the member is absent, empty, or carries only
/// empty lists.
///
/// RECORDED constant, and the whole message: the service states that the targets are not
/// defined and names neither the crawler the request asked for nor the lists it looked in,
/// so a client recognises the condition by the wording whichever crawler write reported it.
/// No trailing full stop, as recorded.
pub const CRAWLER_TARGETS_NOT_DEFINED: &str = "Targets are not defined";

/// `CreateCrawler` for a name already taken. NOT recorded: inferred from the
/// recorded `CreateDatabase` duplicate, whose already-exists wording states the
/// resource kind and withholds the name. What is pinned here is the behaviour — 400,
/// `AlreadyExistsException`, and no disclosure of the requested name.
pub const CRAWLER_ALREADY_EXISTS: &str = "Crawler already exists.";

/// `CreateCrawler` (and the `UpdateCrawler` that replaces a crawler's role), and
/// `CreateColumnStatisticsTaskSettings`, for a `Role` this service cannot assume
/// ([`super::glue_role_is_unassumable`]).
///
/// RECORDED constant, and the whole message: it is the service's fixed template for
/// this check, naming neither the role the request sent, the principal that has to be
/// admitted, nor the operation — so a client recognises the refusal by its wording
/// whichever of those writes reported it. No trailing period, as recorded.
pub const ROLE_NOT_ASSUMABLE: &str =
    "Service is unable to assume provided role. Please verify role's TrustPolicy";

/// `CreateMLTransform` (and the `UpdateMLTransform` that replaces a transform's role) for
/// a `Role` this service cannot assume ([`super::glue_role_is_unassumable`]).
///
/// RECORDED template: the fixed text plus ONE variable part, the ARN that addresses the
/// role the request named ([`super::glue_role_reference`]) — which is the value as sent
/// where `Role` named an ARN, and the ARN of a role of this account where it named a bare
/// name, since the sentence states an `arn:`. The machine learning transform surface words
/// this refusal its own way, which is why it is not the crawler's constant above.
pub fn ml_transform_role_not_assumable(role_arn: &str) -> String {
    format!("Unable to assume role for arn: {role_arn}")
}

/// `CreateMLTransform` for a find-matches transform whose input record tables resolve to a
/// schema narrower than the algorithm can be defined over
/// ([`super::ComAmazonawsGlueAwsglueState::ml_transform_schema_is_too_narrow`]).
///
/// RECORDED constant, and the whole message: the service's fixed sentence for this check,
/// which states neither the number of columns it found, the minimum it wanted nor the table
/// it read the schema out of — so its wording is all a client has to recognise this refusal
/// by, since it shares the operation's `InvalidInputException` with every other input
/// refusal. No trailing period, as recorded.
pub const ML_TRANSFORM_SCHEMA_TOO_NARROW: &str =
    "Number of columns in schema less than the minimum allowed number";

/// `CreateSession` for a `Role` this service cannot assume
/// ([`super::glue_role_is_unassumable`]).
///
/// RECORDED template: the session surface states the requirement as a sentence naming the
/// role the request sent (`CreateSession$Role` is modelled as a role ARN outright), and the
/// component that raised it appends its own diagnostic clause ([`with_diagnostic`]) after
/// the line break the sentence ends with. A session is provisioned by the job executor
/// acting as that role, which is the component the clause names.
pub fn session_role_not_assumable(role_arn: &str) -> String {
    with_diagnostic(
        &format!("{}\n", assume_role_permissions_sentence(role_arn)),
        DIAGNOSTIC_SERVICE_JOB_EXECUTOR,
        DIAGNOSTIC_STATUS_VALIDATION,
        DIAGNOSTIC_CODE_INVALID_INPUT,
    )
}

/// `JobRun$ErrorMessage` of a run whose job's `Role` Glue cannot assume
/// ([`super::glue_role_is_unassumable`]), which is a run that was never dispatched.
///
/// RECORDED template: the job executor reports a run it could not execute as ONE line
/// naming the run it is about — the account the run was made in, the job definition it is a
/// run of and the run's own id — then the exception that stopped it, which here is the SAME
/// sentence the session surface states the requirement with ([`session_role_not_assumable`]):
/// both are the job executor resolving the role the work would be performed as. The sentence
/// ends in the line break that component's messages end in, and the clause naming the
/// component, the status and the code it raised the failure with follows it
/// ([`with_diagnostic`]).
///
/// Every variable part is read off the run and the emulator's own state: the account id this
/// emulator serves, the `JobName` and `JobRunId` of the run, the ARN that addresses the role
/// the job names ([`super::glue_role_reference`]), and the clause's own request id.
pub fn job_run_role_not_assumable(job_name: &str, run_id: &str, role_arn: &str) -> String {
    with_diagnostic(
        &format!(
            "AccountId:{} and JobName:{job_name} and JobRunId:{run_id} failed to execute \
             with exception {}\n",
            ::engine::emulator_account_id(),
            assume_role_permissions_sentence(role_arn)
        ),
        DIAGNOSTIC_SERVICE_JOB_EXECUTOR,
        DIAGNOSTIC_STATUS_VALIDATION,
        DIAGNOSTIC_CODE_INVALID_INPUT,
    )
}

/// The sentence this service states the assume-role requirement with, naming the role it is
/// about: shared by every surface that reports it, so one requirement is worded once.
fn assume_role_permissions_sentence(role_arn: &str) -> String {
    format!("Role {role_arn} should be given assume role permissions for Glue Service.")
}

/// A ruleset write (`CreateDataQualityRuleset`, and the `UpdateDataQualityRuleset` that
/// replaces the stored DQDL) whose `Ruleset` declares no rule at all
/// ([`super::dqdl_rule_count`] answers 0): text no terminated `Rules = [ … ]` list can be
/// read out of, or a list holding no entry.
///
/// RECORDED constant, and the whole message: it names neither the ruleset nor the text it
/// was read from, so a client recognises the refusal by its wording. No trailing period, as
/// recorded.
pub const DATA_QUALITY_RULES_NOT_PARSED: &str = "DataQuality rules cannot be parsed";

/// `CreateGlueIdentityCenterConfiguration` whose `InstanceArn` names no Identity Center
/// instance of this account ([`super::identity_center_instance_is_absent`]).
///
/// RECORDED constant, and the whole message: it names the member in the service's own
/// camel-case spelling and withholds the value the request sent, so the refusal discloses
/// nothing of the request back to it.
pub const IDENTITY_CENTER_INSTANCE_NOT_IN_ACCOUNT: &str =
    "Provided identityCenterInstanceArn is invalid. Please check IdentityCenter instance \
     Arn in your account.";

/// Any operation refusing a request member whose value is outside the form that member
/// admits — the `Versions` of `DeleteSchemaVersions` that spells no version number or
/// range of them ([`super::parse_version_span`]).
///
/// RECORDED template: fixed text plus ONE variable part, the MEMBER's name as the model
/// spells it. The value the caller sent is withheld, so the refusal discloses nothing of
/// the request back to it.
pub fn parameter_value_not_valid(parameter: &str) -> String {
    format!(
        "The parameter value contains one or more characters that are not valid. \
             Parameter Name: {parameter}"
    )
}

/// Any schema-registry operation refusing a member whose value is outside the `@length`
/// its modelled type declares — the `SchemaDefinition` of a schema create, a version
/// registration or a validity check, which targets `SchemaDefinitionString` (length
/// 1..170000).
///
/// RECORDED template: the fixed sentence plus ONE variable part, the PARAMETER the registry
/// names the member by. It states both ends of the bound at once, so a value past the
/// maximum and one short of the minimum read alike, and it echoes back nothing of the value
/// the request sent — which for this member is a document of up to 170000 characters.
pub fn parameter_length_outside_range(parameter: &str) -> String {
    format!("The resource name contains too many or too few characters. Parameter Name: {parameter}")
}

/// A schema-registry LISTING whose `MaxResults` is outside the `@range` its modelled type
/// declares (`MaxResultsNumber`, range 1..100).
///
/// RECORDED template: the fixed sentence plus three variable parts — the maximum the member
/// admits, the value nothing at or below which is served (one below the minimum, which is
/// how the registry states the lower end), and the `MaxResults` the request sent.
pub fn max_results_outside_range(maximum: i64, minimum: i64, requested: &str) -> String {
    format!(
        "MaxResults cannot be greater than {maximum} (or) less than or equal to {}. \
         MaxResults: {requested}",
        minimum.saturating_sub(1)
    )
}

/// `DeleteSchemaVersions` whose `Versions` names the schema set's check-pointed
/// version. Stated by the operation: "When the range of version numbers contain check
/// pointed version, the API will … not proceed with the deletion. You have to remove
/// the checkpoint first using the DeleteSchemaCheckpoint API before using this API."
///
/// RECORDED constant, and the whole message: the refusal states the collision in fixed
/// words and echoes back neither the check-pointed version nor the span the request named.
pub const CHECKPOINT_VERSION_NOT_DELETABLE: &str = "Cannot delete checkpoint version.";

/// `UpdateSchema` that asks for a new `Compatibility` without naming the
/// `SchemaVersionNumber` the new mode is check-pointed at. Stated by the operation:
/// "Version number required for check pointing".
///
/// RECORDED constant, and the whole message: it names neither the schema set nor the mode
/// the request asked for.
pub const SCHEMA_VERSION_CHECKPOINT_MISSING: &str = "Schema version checkpoint is missing.";

/// `UpdateSchema` that asks for the `Compatibility` the schema set already has: a
/// compatibility update records "the last time the compatibility mode was changed"
/// (`SchemaCheckpoint`), so a mode that is not a change is nothing to check-point.
///
/// RECORDED constant, and the whole message: it names neither the schema set nor the mode.
pub const COMPATIBILITY_ALREADY_SET: &str = "Cannot update to the same compatibility setting.";

/// `DeleteSchemaVersions` whose `Versions` names the first version of the schema set.
/// Stated by the operation: "You cannot use the DeleteSchemaVersions API to delete the
/// first schema version in the schema set. The first schema version can only be deleted
/// by the DeleteSchema API."
///
/// NOT recorded: what is pinned is the behaviour — the operation's modelled
/// `InvalidInputException`, with nothing deleted — and a sentence stating that
/// documented requirement, carrying the version the request named.
pub fn first_schema_version_not_deletable(version_number: i64) -> String {
    format!(
        "Schema version {version_number} is the first version of this schema and can only \
             be deleted by the DeleteSchema API."
    )
}

/// A classifier READ (`GetClassifier`, and the `UpdateClassifier` that addresses one
/// the same way) for a name no classifier in this account has.
///
/// Recorded template, reproduced as the service words this error class and NOT
/// shortened: the sentence names the classifier the request addressed — the only
/// variable part, read off the request — and then states the miss a second time,
/// because the service composes its per-resource sentence with its generic one. A
/// client matching on the refusal sees that doubled wording, so dropping the second
/// half would be a divergence rather than a tidy-up.
pub fn classifier_not_found(name: &str) -> String {
    format!("Classifier {name} not found. not found.")
}

/// The DELETE of a classifier this account holds none of (`DeleteClassifier`).
///
/// Recorded template, and a DIFFERENT sentence from the read's above — which is why
/// the two are not one shared string, exactly as the connection read, update and delete
/// each keep their own ([`connection_not_found`], [`connection_not_found_for_update`],
/// [`CONNECTION_NOT_FOUND_FOR_DELETE`]): the
/// delete states the miss ONCE, over the whole address it resolved — the account whose
/// namespace was searched and the requested name, joined by a colon, because a
/// classifier name is unique per account. The name is read off the request, exactly as
/// sent; the account is the one this emulator serves, as
/// [`custom_entity_type_not_found`] reads it.
pub fn classifier_not_found_for_delete(name: &str) -> String {
    format!(
        "Classifier {}:{name} not found.",
        ::engine::emulator_account_id()
    )
}

/// `CreateClassifier` for a name already taken.
///
/// Recorded template, reproduced as the service words this error class and NOT
/// shortened: the sentence names the classifier the create asked for — the only
/// variable part, read off the request — and then states the collision a second time,
/// for the same reason the read's miss above states it twice.
pub fn classifier_already_exists(name: &str) -> String {
    format!("Classifier {name} already exists. already exists")
}

/// `CreateCustomEntityType` for a `Name` this account already holds a custom pattern
/// under. Recorded constant, and the whole message: the service states the collision
/// over the resource kind and the namespace a custom pattern's name is unique in, and
/// echoes back neither the name the create asked for nor the account.
pub const CUSTOM_ENTITY_TYPE_ALREADY_EXISTS: &str =
    "A Custom Entity Type with the same name exist in this account";

/// `CreateClassifier` naming none of the four classifier members. The kind to create
/// is "which field of the request is present" (`CreateClassifier`), so a request
/// naming no field asks for no classifier at all; the sentence states that
/// requirement over the four members, the way the schema-registry
/// [`super::REGISTRY_ID_REQUIRES_ONE`] states its own.
pub const CLASSIFIER_REQUIRES_ONE: &str = "One of GrokClassifier, XMLClassifier, \
         JsonClassifier or CsvClassifier has to be provided.";

/// `CreateClassifier` naming SEVERAL of them, which names no one kind either: the
/// kind is decided by which field is present, so two present fields describe two
/// different classifiers under one name.
pub const CLASSIFIER_ALLOWS_ONLY_ONE: &str = "Only one of GrokClassifier, \
         XMLClassifier, JsonClassifier or CsvClassifier may be provided.";

/// A CSV classifier write whose `QuoteSymbol` is the character its `Delimiter`
/// already is. Stated by the model: "A custom symbol to denote what combines content
/// into a single column value. It must be different from the column delimiter"
/// (`CsvClassifier$QuoteSymbol`).
pub const CSV_QUOTE_SYMBOL_MUST_DIFFER: &str =
    "QuoteSymbol must be different from the column delimiter.";

/// `UpdateClassifier` naming a kind the stored classifier of that name is not: the
/// kind a request is about is the field it carries ("Modifies an existing classifier
/// … depending on which field is present"), and a stored classifier's kind is fixed
/// at create, so such a request describes a classifier that does not exist. NOT
/// recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and a sentence built from the request: the name it
/// addressed and the two kinds, each spelled as the model's member for it.
pub fn classifier_kind_mismatch(name: &str, stored: &str, requested: &str) -> String {
    format!("Classifier {name} is a {stored} and cannot be updated as a {requested}.")
}

/// A custom entity type lookup for a name this account holds no custom pattern
/// under — the miss of every operation that addresses one by `Name`
/// (`GetCustomEntityType`, `DeleteCustomEntityType`).
///
/// Recorded template: the sentence names the pattern the request addressed and the
/// account whose namespace was searched, because a custom pattern's name "must be
/// unique per Amazon Web Services account" (`CustomEntityType$Name`) and so the
/// account is part of the address that resolved to nothing. The name is read off the
/// request, exactly as sent; the account is the one this emulator serves, not a value
/// the request carries.
pub fn custom_entity_type_not_found(name: &str) -> String {
    format!(
        "An existing Custom Entity Type: {name} cannot be found for Account Id {}",
        ::engine::emulator_account_id()
    )
}

/// Any catalog operation whose `CatalogId` names a Data Catalog of ANOTHER account
/// that no resource-based policy grants the caller
/// ([`super::foreign_catalog_refusal`]). Recorded template: it names the calling
/// principal, the IAM action the operation performs and the ARN of the catalog the
/// request addressed, each read off the request and this emulator's own identity.
pub fn not_authorized_on_catalog(action: &str, catalog_arn: &str) -> String {
    let prefix = super::IAM_ACTION_PREFIX;
    format!(
        "User: {} is not authorized to perform: {prefix}{action} on resource: \
             {catalog_arn} because no resource-based policy allows the {prefix}{action} \
             action",
        super::request_principal()
    )
}

/// A trigger lookup (`GetTrigger`, and every other operation that addresses a trigger
/// by name) for a name no trigger of the request's Region has.
///
/// RECORDED constant, and the whole message: the sentence is fixed and names no trigger,
/// so a miss discloses nothing about the Region's namespace, and every operation that
/// addresses a trigger by name reports the miss with this one sentence.
pub const TRIGGER_NOT_FOUND: &str = "Trigger not found.";

/// A trigger lifecycle call whose transition is not defined from the `TriggerState` the
/// addressed trigger is in — `StopTrigger` for a trigger that is not started
/// ([`super::TRIGGER_STATE_ACTIVATED`]).
///
/// RECORDED template: the fixed text is the whole sentence up to and including the colon,
/// and the one variable part is the state the trigger is in AT THE TIME OF THE CALL, read
/// off the stored record rather than off the request. Neither the trigger's name nor the
/// operation is part of the sentence, so the refusal discloses nothing beyond the
/// lifecycle position the caller can already read with `GetTrigger`.
pub fn trigger_state_not_allowed(state: &str) -> String {
    format!("The operation is not allowed when trigger is in state: {state}")
}

/// `CreateTrigger` for a name the request's Region already holds a trigger under, where
/// the definition submitted is the stored one. NOT
/// recorded: inferred from the recorded `CreateDatabase` duplicate, like
/// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
/// the requested name.
pub const TRIGGER_ALREADY_EXISTS: &str = "Trigger already exists.";

/// A create for a resource name already taken whose request asks for a DIFFERENT
/// resource than the stored one, on an operation that models
/// `IdempotentParameterMismatchException` ("The same unique identifier was associated
/// with two different records") and so treats that name as the create's idempotency
/// identifier.
///
/// RECORDED template: fixed text with ONE variable part, the RESOURCE kind as the
/// service names it. It discloses neither the name the request addressed nor which
/// member of the definition differs.
pub fn already_submitted_with_different_configuration(resource: &str) -> String {
    format!("{resource} already submitted with different configuration.")
}

/// A trigger definition whose `Actions` list is absent or empty, on the create that
/// would store it or the update that would overwrite one with it, and every other
/// member a trigger's `Type` requires it to carry: a `Schedule` for a scheduled
/// trigger ("This field is required when the trigger type is SCHEDULED" —
/// `CreateTriggerRequest$Schedule`) and a `Predicate` for a conditional one ("This
/// field is required when the trigger type is CONDITIONAL").
///
/// RECORDED template, with no trailing period: the refusal names the MEMBER that has
/// to carry something and says that neither no value nor an empty one will do, and it
/// discloses neither the trigger nor the operation nor the type the request named — so
/// one sentence serves every definition member this service judges, whichever write
/// submitted it.
pub fn trigger_member_required(member: &str) -> String {
    format!("{member} cannot be null or empty")
}

/// A trigger definition whose `Schedule` is not the `cron(..)` expression the member
/// documents ("a `cron` expression used to specify the schedule",
/// `CreateTriggerRequest$Schedule`), on the create that would store it or the update that
/// would overwrite one with it ([`super::is_cron_expression`]).
///
/// RECORDED template: the fixed text `The schedule ` and ` is invalid.` around ONE variable
/// part, the expression the request sent, taken verbatim. The TRIGGER surface words this
/// defect its own way; the crawler surface keeps its own sentence
/// ([`schedule_not_a_cron_expression`]), so a client can tell which surface refused its
/// schedule.
pub fn trigger_schedule_invalid(expression: &str) -> String {
    format!("The schedule {expression} is invalid.")
}

/// A CONDITIONAL trigger definition naming one job both in a condition of its
/// `Predicate` and in one of its `Actions` ([`super::job_is_predicate_and_action`]).
///
/// RECORDED constant, and the whole message: it states the rule the definition breaks
/// and names neither the job it was broken over nor the trigger, so a client sees the
/// same sentence whichever job it doubled up. It is a refusal of the DEFINITION, so it
/// is answered whether or not that job exists.
pub const JOB_IN_PREDICATE_AND_ACTION: &str =
    "Job cannot be a Predicate and Action for Conditional Trigger.";

/// A trigger create that asks for a type that cannot be started on creation ("True is
/// not supported for ON_DEMAND triggers" — `CreateTriggerRequest$StartOnCreation`).
/// The sentence is that modelled requirement, with the type the request named.
pub fn start_on_creation_not_supported(trigger_type: &str) -> String {
    format!("StartOnCreation true is not supported for {trigger_type} triggers")
}

/// A trigger create whose `Predicate` lists several conditions without a `Logical`
/// ("An optional field if only one condition is listed. If multiple conditions are
/// listed, then this field is required" — `Predicate$Logical`). The sentence is that
/// modelled requirement, with the number of conditions the request listed.
pub fn predicate_logical_required(conditions: usize) -> String {
    format!("Logical is required when a Predicate lists {conditions} conditions")
}

/// Any operation whose `JobName`/`Name` names no job definition of the Region it is
/// served in — a read or a write that addresses a job (`GetJob`, `UpdateJob`,
/// `GetJobRun`, `GetJobRuns`, `BatchStopJobRun`), and the job an action of a trigger
/// would run. NOT the bookmark surface, which addresses a job's CONTINUATION point
/// rather than its definition and reports its own miss for every name that resolves
/// none ([`job_continuation_not_found`]).
///
/// Recorded constant: the job-definition miss is FIXED text, trailing period
/// included, and has no variable part — the refusal names neither the requested job,
/// nor the Region, nor the resource that referred to the job. One constant, shared by
/// every job-definition lookup, so a client matching on the sentence recognises the
/// miss whichever operation reported it.
pub const JOB_NOT_FOUND: &str = "Job not found.";

/// A job definition or a job run that asks for its capacity BOTH ways: "do not set
/// `MaxCapacity` if using `WorkerType` and `NumberOfWorkers`"
/// (`CreateJobRequest$MaxCapacity`), the requirement `CreateJob`, `UpdateJob` and
/// `StartJobRun` each hold their request to.
///
/// Recorded constant, and the whole message: entirely fixed text with no variable part,
/// which states the two capacity members in the service's SPACED-OUT prose spelling rather
/// than in the model's member spelling, and ends in a period. One constant, shared by every
/// operation that takes a capacity, so a client matching the sentence recognises the refusal
/// whichever of them reported it.
pub const MAX_CAPACITY_EXCLUDES_WORKERS: &str =
    "Please do not set Max Capacity if using Worker Type and Number of Workers.";

/// A job DEFINITION that states its capacity as workers and states only HALF of it: the kind
/// of worker without how many, or how many without which kind. "Instead, you should specify a
/// `Worker type` and the `Number of workers`" (`CreateJobRequest$MaxCapacity`) — the two
/// together are one capacity, so a definition carrying one of them alone sizes no job.
///
/// Recorded constant, and the whole message: entirely fixed text with no variable part,
/// stating the two members in the same SPACED-OUT prose spelling
/// [`MAX_CAPACITY_EXCLUDES_WORKERS`] states them in, and ending in a period. One constant,
/// shared by the writes that carry a whole job definition, so a client matching the sentence
/// recognises the refusal whichever of them reported it.
pub const WORKER_TYPE_AND_COUNT_TOGETHER: &str =
    "Please set both Worker Type and Number of Workers.";

/// A job DEFINITION that allocates fewer workers of its `WorkerType` than the service admits
/// for that type ([`super::min_workers_for_worker_type`]).
///
/// RECORDED template: fixed text around TWO variable parts — the worker type the definition
/// named, echoed as it was sent, and the floor the service publishes for it, which is the
/// value the check was made against rather than a number written into the sentence. Named in
/// the same SPACED-OUT prose spelling the other capacity refusals use.
pub fn worker_count_below_minimum(worker_type: &str, minimum: i32) -> String {
    format!(
        "To use Worker Type {worker_type}, minimum allowed value of Number of Workers is \
         {minimum}."
    )
}

/// A job DEFINITION whose `JobCommand` names the command the job runs and no script for it to
/// run ([`super::script_location_defect`]).
///
/// RECORDED template: fixed text around ONE variable part, the command the definition named,
/// taken from the request. It states both ways the member carries no value — absent and empty
/// — in one sentence, because one check covers both, and it closes with no period, which is
/// how the recorded response words it.
pub fn script_location_null_or_empty(command_name: &str) -> String {
    format!("Script location cannot be null or empty for {command_name} command")
}

/// A job DEFINITION that gives a worker configuration to a command the service does not size
/// in workers ([`super::command_takes_a_worker_configuration`]).
///
/// RECORDED template: fixed text around ONE variable part, the command the definition named,
/// taken from the request, and closing with no period. It names the `WorkerType` member in
/// the same SPACED-OUT prose spelling the other capacity refusals use.
pub fn worker_type_unsupported_for_command(command_name: &str) -> String {
    format!("Worker Type is not supported for Job Command {command_name}")
}

/// `CreateCrawler` for an S3 target whose `EventQueueArn` (or `DlqEventQueueArn`) names a
/// queue the crawler's `Role` is shown nothing about: its policies admit no call of that
/// queue's own service on it ([`super::event_queue_refusal`]).
///
/// Recorded template, and the WHOLE message: one fixed sentence around one variable part, the
/// queue ARN exactly as the request supplied it. It CONFLATES the two reasons a crawl could
/// not read that queue — the queue not being there, and the role not being allowed to reach
/// it — because a role that may not touch the queue at all cannot be told apart from a queue
/// that is not there. It names no queue call: there is no grant to ask for on a queue whose
/// existence this role cannot establish. Glue states its own refusal rather than restating
/// the queue service's authorization answer: it discloses neither the identity the crawl
/// would run as, nor which of IAM's rules withheld the grant, so the refusal reads the same
/// whichever of them it was.
pub fn event_queue_inaccessible(queue_arn: &str) -> String {
    format!("SQS queue {queue_arn} does not exist or the role provided does not have access to it.")
}

/// `CreateCrawler` for an S3 target whose event queue the crawler's `Role` is shown something
/// about — its policies admit some call of that queue's own service on it — but may not
/// consume ([`super::event_queue_refusal`]).
///
/// Recorded in part: what is pinned is that this refusal NAMES the queue action the role is
/// missing, so a caller whose role can reach the queue is told which grant to add rather than
/// sent looking for a queue that is demonstrably there. Stated as the sentence of
/// [`event_queue_inaccessible`] — which a client matching Glue's refusal on a queue it cannot
/// read still recognises — followed by the REQUIREMENT the crawl holds that queue to, naming
/// the queue calls a crawl in event mode makes on it ([`super::EVENT_QUEUE_ACTIONS`], in the
/// order it makes them). The variable part is the same single one: the queue ARN exactly as
/// the request supplied it; the identity the crawl would run as is not disclosed here either.
pub fn event_queue_grant_missing(queue_arn: &str) -> String {
    format!(
        "{} Please make sure the role has {} permissions on the queue.",
        event_queue_inaccessible(queue_arn),
        super::event_queue_actions_clause()
    )
}

/// `CreateCrawler` for a DynamoDB target whose table the crawler's `Role` may not read
/// ([`super::dynamodb_target_refusal`]).
///
/// Stated the way this service states the same refusal about the other store it reaches a
/// target through ([`event_queue_grant_missing`]): one sentence that CONFLATES the table not
/// being there with the role not being allowed to read it, because a role whose policies
/// withhold those reads cannot be shown the table is there, then the REQUIREMENT the crawl
/// holds the table to — the table calls a crawl makes on it
/// ([`super::DYNAMODB_TARGET_ACTIONS`], in the order it makes them) — and last the diagnostic
/// clause this service appends to a refusal its framework raised ([`with_diagnostic`]), which
/// states the component, the status, the code and the id of the request it refused.
///
/// Its variable parts are read off the request and the clause's own per-call id: the table
/// the target named, exactly as the request supplied it, and the request id. The identity the
/// crawl would run as is not disclosed, and neither is which of IAM's rules withheld the
/// grant.
pub fn dynamodb_target_grant_missing(table: &str) -> String {
    with_diagnostic(
        &format!(
            "DynamoDB table {table} does not exist or the role provided does not have access \
             to it. Please make sure the role has {} permissions on the table.",
            super::dynamodb_target_actions_clause()
        ),
        DIAGNOSTIC_SERVICE_GLUE,
        DIAGNOSTIC_STATUS_VALIDATION,
        DIAGNOSTIC_CODE_INVALID_INPUT,
    )
}

/// A job run addressed by the (job name, run id) PAIR that this endpoint holds no run
/// under — the read (`GetJobRun`), the retry a start names (`StartJobRun$JobRunId`) and
/// each entry of a stop the job holds no such run for (`BatchStopJobRun`).
///
/// Recorded CONSTANT, and the whole message: the service states the pair it could not
/// resolve a run for — the job name and the run id — in fixed words and echoes back
/// neither of them, so the refusal a client matches on is the same whatever was
/// asked. Shared by every job-run lookup, so one operation's miss cannot drift from
/// another's — and it is the refusal for EITHER half of the pair being unknown to the
/// read, which resolves the run by the pair rather than the job first
/// ([`crate::ops::get_job_run`]); only an operation that addresses the job DEFINITION
/// reports the job itself as missing ([`JOB_NOT_FOUND`]).
pub const JOB_RUN_NOT_FOUND: &str = "Job run not found for the given job name and runId.";

/// Each `BatchStopJobRun` entry for a run the named job DOES hold but whose current
/// `JobRunState` leaves no stop to submit — a run that has already finished
/// ([`crate::state::TERMINAL_JOB_RUN_STATES`]).
///
/// Recorded CONSTANT, and the whole message: the service states that the run's state is
/// what stands in the way, in fixed words, and echoes back neither the run id nor the
/// state it is in — the entry AROUND the detail is what names the job and the run
/// ([`crate::ops::batch_stop_job_run`]). This is a STATE conflict rather than a bad
/// request, so the entry's `ErrorCode` names the "cannot be stopped" error class and not
/// the operation's input-validation one.
pub const JOB_RUN_CANNOT_BE_STOPPED: &str = "Job Run cannot be stopped in current state.";

/// A bookmark operation (`GetJobBookmark`, `ResetJobBookmark`) that resolves no bookmark:
/// a job no run of has written one yet, a job whose run the request's `RunId` names no run
/// of, and a `JobName` this Region holds no job definition under at all — the surface
/// addresses a CONTINUATION point rather than the job, so all three are the one miss.
///
/// RECORDED template: the fixed text `Continuation for job ` and ` not found` — no trailing
/// period — around its one variable part, the `JobName` the request sent, taken exactly as
/// sent.
pub fn job_continuation_not_found(job_name: &str) -> String {
    format!("Continuation for job {job_name} not found")
}

/// `GetDashboardUrl` for a `ResourceType` the operation does not serve a dashboard
/// for. Recorded template: the refusal names the operation and the resource type the
/// request asked for, which is the member it is refused over.
pub fn dashboard_url_not_supported(resource_type: &str) -> String {
    format!("GetDashboardUrl operation is not supported for {resource_type}")
}

/// `GetDashboardUrl` for a `ResourceId` that names no resource of the `ResourceType` the
/// request asked a dashboard for.
///
/// RECORDED constant, and the whole message: the sentence is the fixed, generic one this
/// operation answers a miss with — it names neither the id the request addressed nor the
/// kind of resource it was looked for as, and carries no trailing period.
///
/// It reads the same as the WORKFLOW surface's own miss ([`WORKFLOW_NOT_FOUND`]) and is a
/// separate constant all the same: the two surfaces are recorded independently, so either
/// may be re-worded without the other.
pub const DASHBOARD_URL_RESOURCE_NOT_FOUND: &str = "Entity not found";

/// The miss of the TAGGING WRITES: a `ResourceArn` that is the ARN of a taggable Glue
/// resource but names none this endpoint holds — `TagResource` and `UntagResource` — and the
/// tag READ's own answer for every kind whose own sentence no recording states
/// ([`crate::state::tag_read_not_found_message`]).
///
/// RECORDED constant, and the whole message: this fixed, generic sentence names neither the
/// ARN it was handed, nor the kind of resource it was looked for as, nor the name inside it,
/// and carries no trailing period. For the WRITES it is the answer whatever the kind is,
/// which is what distinguishes "this resource is not here" from "this is not a resource I
/// tag" (the latter being the operation's `InvalidInputException`, which carries no prose at
/// all — [`crate::state::INVALID_RESOURCE_ARN_MESSAGE`]). The READ resolves the ARN through
/// the surface that owns the kind, so it answers this sentence only where that surface's own
/// miss is recorded to read this way — the workflow's, whose surface answers exactly this
/// text ([`WORKFLOW_NOT_FOUND`]).
///
/// It is NOT the sentence a tagging call answers for an ARN under which this service places
/// no entity AT ALL, which no owning surface words
/// ([`UNPLACEABLE_RESOURCE_ARN_NOT_FOUND`], recorded in a different case).
pub const TAGGED_RESOURCE_NOT_FOUND: &str = "Entity not found";

/// The miss a tagging call answers for a `ResourceArn` that ADDRESSES a Glue resource
/// ([`crate::state::is_glue_resource_address`]) whose resource TYPE this service places no
/// entity under at all: a resource type it dispatches no lookup for (a catalog table, a table
/// version, a user defined function, a partition — none of which carries a tag set), and a
/// resource portion that names a type but no one resource of it.
///
/// RECORDED constant, and the whole message: the surface answers its modelled
/// `EntityNotFoundException` with this fixed sentence — it names neither the ARN it was
/// handed, nor a resource type, nor a name, and carries no trailing period. There is no
/// owning surface to take a sentence from, which is why this is a constant and not a
/// per-kind lookup, and why it is a SEPARATE constant from the surface's miss of a kind it
/// does place ([`TAGGED_RESOURCE_NOT_FOUND`]): the two are recorded independently and the
/// recordings differ in case.
///
/// A value that addresses no Glue resource at all — no Glue resource ARN, or one whose
/// resource portion names no Glue resource type — is no entity lookup to begin with: it earns
/// the operation's `InvalidInputException`, which carries no prose
/// ([`crate::state::INVALID_RESOURCE_ARN_MESSAGE`]).
///
/// It reads the same as the catalog's own table read's miss ([`TABLE_ENTITY_NOT_FOUND`]) and
/// is a separate constant all the same, for the reason every other repeated sentence of this
/// module is: each surface is recorded independently.
pub const UNPLACEABLE_RESOURCE_ARN_NOT_FOUND: &str = "Entity Not Found";

/// A session lookup addressed by `Id` for an id this account holds no session under —
/// `GetSession`'s miss, and the miss of every operation that resolves a session the same
/// way and words it the same.
///
/// RECORDED template: fixed text around its one variable part, the id the request
/// addressed, taken from the request exactly as sent, with a trailing period.
///
/// It is NOT the wording of the STATEMENT surface's own session miss
/// ([`session_id_not_found`]): an operation that resolves a `SessionId` in order to reach
/// the statements of that session states the member it could not resolve, which is why the
/// two are two strings and not one.
pub fn session_not_found(id: &str) -> String {
    format!("Session {id} not found.")
}

/// The session miss of an operation that addresses a session's STATEMENTS through its
/// `SessionId` (`GetStatement`, `ListStatements`) for an id this account holds no session
/// under.
///
/// RECORDED template: fixed text around its one variable part, the `SessionId` the request
/// sent, taken exactly as sent — the sentence names the MEMBER that resolved to nothing
/// ("Session ID"), and carries no trailing period, where the session read's own miss
/// ([`session_not_found`]) names the resource and ends in one.
pub fn session_id_not_found(session_id: &str) -> String {
    format!("Session ID {session_id} not found")
}

/// `RunStatement` for a `SessionId` that resolves to no session at all — an id nothing
/// created, and one whose session has been deleted and its record fully removed.
///
/// RECORDED constant, and the whole message: the sentence this operation refuses an
/// unresolvable session with is the service's fixed, generic entity miss. It has NO
/// variable part — neither the id the request sent nor the member it was sent under
/// appears in it — and carries no trailing period.
///
/// It is therefore neither the session read's own miss ([`session_not_found`]) nor the
/// statement-listing surface's ([`session_id_not_found`]): this service words the miss per
/// operation, and the statement RUN is the operation recorded answering the generic
/// sentence. It reads the same as the WORKFLOW surface's miss ([`WORKFLOW_NOT_FOUND`]) and
/// is a separate constant all the same, for the same reason that one is: the surfaces are
/// recorded independently, so either may be re-worded without the other.
pub const RUN_STATEMENT_SESSION_NOT_FOUND: &str = "Entity not found";

/// A read of the Glue Identity Center configuration of an account that has none
/// (`GetGlueIdentityCenterConfiguration`), and the delete or update of the same absent
/// singleton.
///
/// Recorded constant: the refusal names the Identity Center application Glue could not
/// find and states what has to happen first; it has no variable part, because the
/// account is the whole address of the configuration.
pub const IDENTITY_CENTER_CONFIGURATION_NOT_FOUND: &str =
    "Idc application not found. Need to create glue application first.";

/// `CreateGlueIdentityCenterConfiguration` for an account that already has one.
///
/// RECORDED template: fixed text plus ONE variable part, the `ApplicationArn` of the
/// configuration the account already holds, read off the stored record — so the caller
/// losing the race for the singleton learns which application owns it. The service names
/// the application the way it names it in this sentence (`Glue Idc Application`), as the
/// miss of the same singleton does ([`IDENTITY_CENTER_CONFIGURATION_NOT_FOUND`]).
pub fn identity_center_configuration_already_exists(application_arn: &str) -> String {
    format!("Glue Idc Application already exists. ApplicationArn: {application_arn}")
}

/// `CreateGlueIdentityCenterConfiguration` whose `InstanceArn` is not the ARN of an
/// Identity Center instance ([`super::identity_center_instance_id`]). NOT recorded:
/// what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and a sentence carrying the value the request sent plus
/// the form the member has to take, as [`malformed_version_id`] does for its own.
pub fn malformed_identity_center_instance_arn(instance_arn: &str) -> String {
    let segment = super::IDENTITY_CENTER_INSTANCE_SEGMENT;
    format!(
        "InstanceArn {instance_arn} is not the ARN of an Identity Center instance, \
             which names its instance as arn:aws:sso:::{segment}<instance id>"
    )
}

/// A table-optimizer READ (`GetTableOptimizer`) for a type the addressed table has no
/// optimizer of.
///
/// RECORDED template: the sentence names the optimizer type and then the table the
/// request addressed, which together are the whole address of an optimizer, and it is
/// followed by the diagnostic clause this service appends to such a refusal
/// ([`with_diagnostic`]) — stating the 404 the catalog raised the miss with, even
/// though the call itself is answered with the `EntityNotFoundException` shape's own
/// status.
pub fn table_optimizer_not_found(table_name: &str, optimizer_type: &str) -> String {
    with_diagnostic(
        &format!("No optimizer of type {optimizer_type} found for the table: {table_name}"),
        DIAGNOSTIC_SERVICE_GLUE,
        DIAGNOSTIC_STATUS_NOT_FOUND,
        DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
    )
}

/// The DELETE (`DeleteTableOptimizer`) for the same miss. Recorded template, and a
/// DIFFERENT sentence from the read's above — which is why the two are not one shared
/// string: it states that no optimizer of that type exists ON the table, carrying the
/// same two variable parts and the same diagnostic clause.
pub fn table_optimizer_not_found_for_delete(table_name: &str, optimizer_type: &str) -> String {
    with_diagnostic(
        &format!("An optimizer of type {optimizer_type} does not exist on table {table_name}"),
        DIAGNOSTIC_SERVICE_GLUE,
        DIAGNOSTIC_STATUS_NOT_FOUND,
        DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
    )
}

/// Any table-optimizer operation whose `TableName` names a stored table whose type
/// cannot be optimized — one that is no open-table-format table
/// ([`super::TableEntry::is_iceberg_table`]).
///
/// RECORDED template: the fixed validation sentence plus the diagnostic clause
/// ([`with_diagnostic`]) stating the status and the code the validation was raised
/// with. It is a refusal of the REQUEST rather than a miss, so it names neither the
/// table nor the type the request asked for.
pub fn unsupported_table_type() -> String {
    with_diagnostic(
        "Request Validation Failed: Unsupported Table type",
        DIAGNOSTIC_SERVICE_GLUE,
        DIAGNOSTIC_STATUS_VALIDATION,
        DIAGNOSTIC_CODE_INVALID_INPUT,
    )
}

/// `CreateTableOptimizer` for a table whose parameters name no Iceberg metadata file
/// ([`super::TableEntry::iceberg_metadata_location`]).
///
/// Registering an optimizer reads the table's current metadata file, because that is
/// what names the data files the optimizer would work on — so a table that carries no
/// filepath is refused whatever its declared type is.
///
/// RECORDED template: the fixed sentence plus the diagnostic clause
/// ([`with_diagnostic`]), naming neither the table nor the parameter it was read from.
pub fn table_metadata_filepath_missing() -> String {
    with_diagnostic(
        "Glue table metadata filepath is null",
        DIAGNOSTIC_SERVICE_GLUE,
        DIAGNOSTIC_STATUS_VALIDATION,
        DIAGNOSTIC_CODE_INVALID_INPUT,
    )
}

/// Any table-optimizer operation whose `DatabaseName`/`TableName` resolves to no table
/// of the addressed Data Catalog.
///
/// RECORDED template: this surface resolves the table through the Data Catalog, so what
/// a client is shown is the CATALOG's own refusal of that read — the same constant the
/// catalog's table read answers ([`TABLE_ENTITY_NOT_FOUND`]), carrying the catalog
/// component's diagnostic clause — wrapped in this service's clause, each with its own
/// request id. It names neither the table nor the database, because the catalog's read
/// miss names neither.
pub fn table_optimizer_entity_not_found() -> String {
    with_diagnostic(
        &with_diagnostic(
            TABLE_ENTITY_NOT_FOUND,
            DIAGNOSTIC_SERVICE_DATA_CATALOG,
            DIAGNOSTIC_STATUS_VALIDATION,
            DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
        ),
        DIAGNOSTIC_SERVICE_GLUE,
        DIAGNOSTIC_STATUS_NOT_FOUND,
        DIAGNOSTIC_CODE_ENTITY_NOT_FOUND,
    )
}

/// The component a diagnostic clause names for a refusal THIS service raised.
const DIAGNOSTIC_SERVICE_GLUE: &str = "glue";

/// The component a diagnostic clause names for a refusal the DATA CATALOG raised, which
/// this service surfaces wrapped in its own
/// ([`table_optimizer_entity_not_found`]).
const DIAGNOSTIC_SERVICE_DATA_CATALOG: &str = "AmazonDataCatalog";

/// The component a diagnostic clause names for a refusal the JOB EXECUTOR raised: the
/// component that provisions a job run or an interactive session, and so the one that
/// resolves the role such a run would be performed as ([`session_role_not_assumable`]).
/// As recorded.
const DIAGNOSTIC_SERVICE_JOB_EXECUTOR: &str = "AWSGlueJobExecutor";

/// The `Error Code` a clause states for a reference that resolved to nothing.
const DIAGNOSTIC_CODE_ENTITY_NOT_FOUND: &str = "EntityNotFoundException";

/// The `Error Code` a clause states for a request the service would not accept.
const DIAGNOSTIC_CODE_INVALID_INPUT: &str = "InvalidInputException";

/// The `Status Code` a clause states for a reference that resolved to nothing: the
/// status the component RAISED the miss with, which is not the status the endpoint
/// answers the call with — the awsJson client-error status the modelled shape carries.
/// As recorded, including for the clause the catalog's own refusal is wrapped in.
const DIAGNOSTIC_STATUS_NOT_FOUND: u16 = 404;

/// The `Status Code` a clause states for a refused request, and the one the CATALOG
/// component states for the miss it raises: as recorded.
const DIAGNOSTIC_STATUS_VALIDATION: u16 = 400;

/// `sentence` followed by the diagnostic clause this service appends to a refusal its
/// framework raised:
/// ` (Service: <component>; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)`.
///
/// The request id is the clause's only per-run field: nothing in the request carries
/// one, so it is minted per call from the emulator's one request-id mint
/// (`service_common::query_error_envelope::new_request_id`, a fresh v4 UUID) and is
/// indistinguishable from the one the framework's own renderers report. A clause
/// wrapping another's text states its own component, status and code, so one refusal
/// can carry the inner component's clause and this service's both.
fn with_diagnostic(
    sentence: &str,
    service: &str,
    status_code: u16,
    error_code: &str,
) -> String {
    format!(
        "{sentence} (Service: {service}; Status Code: {status_code}; Error Code: \
             {error_code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// `CreateTableOptimizer` for a type the addressed table already has an optimizer of.
/// NOT recorded: inferred from the recorded `CreateDatabase` duplicate, like
/// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of
/// what the request addressed.
pub const TABLE_OPTIMIZER_ALREADY_EXISTS: &str = "Table optimizer already exists.";

/// A read that has to resolve a table's metadata through the source a FEDERATED
/// database points at ([`super::federated_source_refusal`]).
///
/// NOT recorded as a sentence: what is pinned is the behaviour — 400 with the modelled
/// `FederationSourceRetryableException`, the code that tells a client to retry rather
/// than give up — and a sentence naming the database the request addressed and the
/// connection its registration points the source at, both read off the stored
/// registration.
pub fn federated_source_unreachable(database_name: &str, source: &str) -> String {
    format!(
        "Federated database {database_name} could not be reached through its source \
             {source}."
    )
}

/// A `CreateTable` whose `OpenTableFormatInput` names an open table format with no input of
/// its own, submitted with a `TableInput` that does define the table. The member carries
/// exactly one format (`OpenTableFormatInput$IcebergInput`), so a value carrying none asks
/// for a table in no format; a create that defines no table either is the refusal above
/// ([`MISSING_ICEBERG_CREATE_INPUT`]).
///
/// NOT recorded for this branch, so it is worded by the SAME rule as the recorded one it
/// sits beside: the create names the nested input it was handed none of. Only the input
/// differs — this branch runs out one level higher up, at the format itself.
pub const OPEN_TABLE_FORMAT_REQUIRES_ICEBERG: &str = "IcebergInput cannot be equal to null";

/// A `CreateTable` of an open-table-format table that names no location for it. An
/// Iceberg table's metadata is written under the table's own location, so a create
/// that names none has nowhere to mint it. NOT recorded: what is pinned is the
/// behaviour — 400 with the operation's modelled `InvalidInputException` — and a
/// sentence naming the two members the location may come from.
pub const ICEBERG_TABLE_REQUIRES_LOCATION: &str =
    "An Iceberg table requires a location, in TableInput.StorageDescriptor.Location \
         or in IcebergInput.CreateIcebergTableInput.Location.";

/// A `CreateTable` that carries no input defining the table it asks for: no `TableInput`,
/// and no `CreateIcebergTableInput` under its `OpenTableFormatInput` to define one.
///
/// RECORDED constant, and the whole message: a create naming no definition falls through
/// to the open-table-format path, and the refusal names the nested input THAT path needs
/// and nothing of the request — so one wording answers every such create.
pub const MISSING_ICEBERG_CREATE_INPUT: &str = "CreateIcebergTableInput cannot be equal to null";

/// `UpdateTable` whose `VersionId` is not the version the table is currently under —
/// one a later write has replaced, or one the table never had.
///
/// Recorded constant: the refusal states that the update lost a race and names neither
/// the version the request carried nor the one the table is under, so a client sees the
/// same sentence whichever stale version it sent.
pub const TABLE_UPDATE_CONCURRENT_MODIFICATION: &str =
    "Update table failed due to concurrent modifications.";

/// `StartJobRun` for a `JobName` the endpoint's Region holds no job definition for —
/// a name nothing created, or one created through ANOTHER Region's endpoint.
/// Recorded constant: the start's refusal names the metadata it could not read rather
/// than the miss every other job lookup reports ([`JOB_NOT_FOUND`]), which is why the
/// start does not share that sentence.
pub const JOB_METADATA_MISSING: &str = "Failed to start job run due to missing metadata.";

/// A write that takes a JOB DEFINITION (`CreateJob`, `UpdateJob`) whose definition carries
/// no value for a STRUCTURE member the job cannot run without — the command it runs.
///
/// RECORDED template: fixed text plus ONE variable part, the member the definition left out,
/// spelled as the model spells that member of the definition (`JobUpdate$Command`) rather
/// than as the wire path it travelled under. The members are judged in the order the
/// definition declares them to be needed — the command before the role — so a definition
/// missing both is refused about the command.
///
/// A member whose value is a STRING states the same requirement over both ways a string
/// carries no value ([`job_definition_member_null_or_empty`]): such a member can be sent
/// blank as well as left out, and the one check that covers both says so.
pub fn job_definition_member_null(member: &str) -> String {
    format!("{member} should not be null.")
}

/// The same, for a STRING member of a job definition the job cannot run without — the role
/// it runs under (`JobUpdate$Role`, documented "(required)").
///
/// RECORDED template, with the same one variable part: the sentence states BOTH ways such a
/// member carries no value, because one check covers both — the member absent, and the
/// member sent as a string with nothing in it — so a client reading it is told the rule and
/// not just the case its own request hit.
pub fn job_definition_member_null_or_empty(member: &str) -> String {
    format!("{member} should not be null or empty.")
}

/// A write that takes a JOB DEFINITION (`CreateJob`, `UpdateJob`) whose `GlueVersion` is of
/// the form the member admits but names no version the service runs
/// ([`super::is_supported_glue_version`]).
///
/// RECORDED template: fixed text plus ONE variable part, the version the definition named,
/// echoed exactly as sent — so a client reading the refusal sees which version it asked for.
pub fn glue_version_invalid(glue_version: &str) -> String {
    format!("Glue version: {glue_version} is invalid")
}

/// A tag write whose `ResourceArn` names a Region other than the one the endpoint
/// serving the request is in (`TagResource`). Recorded constant: the refusal states
/// only that the ARN belongs elsewhere and names neither the ARN nor either Region,
/// and it is a client-INPUT refusal rather than a miss — a Glue ARN of another Region
/// is a bad parameter, whether or not that Region holds the resource.
///
/// This is the WRITE's wording and not this crate's answer everywhere: the tag
/// READ (`GetTags`) answers that condition with the kind's own miss
/// ([`crate::state::tag_read_not_found_message`]), as recorded for a job ARN addressed at
/// another Region's endpoint — an ARN of another Region names a resource THIS endpoint
/// holds nothing for, which the read reports as the miss it is.
pub const ARN_WRONG_REGION: &str = "Arn passed doesn't belong to this region.";

/// A listing whose `Expression` is not a regular expression at all
/// (`crate::ops::get_tables`).
///
/// RECORDED template: the service compiles the caller's expression and RELAYS what the
/// engine compiling it said about it — fixed text (" near index ", and the layout that
/// writes the expression on a line of its own under a caret) around three variable parts,
/// every one of them read off the request's own value: what is wrong with it, the index
/// into it the parse stopped at, and the expression itself. The reading of that fault is
/// the workspace's one reading of it
/// ([`::service_common::pattern_syntax_fault`]), so a caller locating a fault in its own
/// expression reads one form wherever this emulator compiles one.
///
/// `None` for a value that IS a regular expression — a pattern this catalog's matcher
/// cannot build is not a syntax fault to place, and carries no index
/// ([`expression_not_matchable`]).
///
/// Nothing of the matcher behind the listing appears in the statement: neither the name of
/// the engine this emulator compiles with, nor the expression this catalog rewrites the
/// caller's into ([`super::translate_name_pattern`]) — the wildcard expansion makes the two
/// texts different lengths, so the fault is placed back at the character of the CALLER's own
/// expression the faulting one came from.
pub fn expression_syntax_fault(pattern: &str) -> Option<String> {
    let read_as = super::translate_name_pattern(pattern);
    let fault = ::service_common::pattern_syntax_fault::pattern_fault(&read_as.translated)?;
    // A fault the parse reached at the END of the text it read belongs one past the last
    // character of the caller's own, which is where such a fault is stated.
    let placed_at = read_as
        .origin
        .get(fault.index())
        .copied()
        .unwrap_or_else(|| pattern.chars().count());
    Some(fault.placed_at(placed_at).stated_in(pattern))
}

/// A listing whose `Expression` IS a regular expression but no matcher this catalog can
/// build: one whose matcher would be larger than the catalog compiles
/// ([`super::MAX_PATTERN_PROGRAM_BYTES`]), or one using a construct its matcher does not
/// implement.
///
/// NOT recorded: no fault can be placed inside such a value — it has none — so what is
/// pinned is the behaviour (400 with the listing's own `InvalidInputException`) and the
/// sentence states the member's value and nothing of the matcher behind it.
pub fn expression_not_matchable(pattern: &str) -> String {
    format!("Expression {pattern} is not a pattern this catalog can match")
}

/// A listing whose `MaxResults` asks for a page smaller than one entry
/// (`crate::ops::get_tables`).
///
/// RECORDED constant: entirely fixed text — neither the value the request sent nor the
/// bound it missed is interpolated, and the member is named as the service names it
/// internally rather than as the wire spells it.
pub const PAGE_SIZE_NOT_POSITIVE: &str = "MaxItems has to be a positive number";

/// A Data Catalog getter handed a `NextToken` the catalog did not issue for that listing
/// ([`super::pagination`]): `GetTables`.
///
/// RECORDED constant: entirely fixed text, with nothing of the value echoed — the token is
/// the caller's and a refusal that quoted it back would be the only place a forged cursor
/// is reflected.
pub const INVALID_TOKEN_VALUE: &str = "Invalid token value";

/// A JOB listing handed a `NextToken` this account's store did not issue for it
/// (`ListJobs`). RECORDED constant, and deliberately NOT the catalog's sentence above:
/// Glue words the same refusal differently per operation, so each states its own.
pub const INVALID_PAGINATION_TOKEN: &str = "Invalid pagination token.";

/// A listing whose `MaxResults` asks for a page LARGER than the service admits at that
/// member (`crate::ops::get_tables`): refused at the request boundary, as a value outside
/// a member's range is, rather than served as a bigger page.
///
/// RECORDED template, and nothing of it is spelled here: the envelope is the cross-service
/// one ([`::service_common::validation_message::envelope_over`]) and the clause and its
/// requirement are the framework's own rendering of a `@range` maximum
/// (`aws_smithy_schema_server::Violation::message`), so this refusal reads exactly as one
/// the validating walk could have reported. The variable parts are the value the request
/// sent, the member it sent it at — spelled as the API spells the parameter — and the
/// largest page that member admits.
pub fn page_size_above_maximum(member_path: &str, requested: i32, maximum: i32) -> String {
    let clause = ::aws_smithy_schema_server::Violation {
        path: member_path.to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::MaxValue(maximum.to_string()),
        // The clause quotes the value the request carried; `MaxResults` carries no
        // `@sensitive`, so there is nothing to withhold.
        value: Some(requested.to_string()),
        sensitive: false,
    }
    .message();
    ::service_common::validation_message::envelope_over(std::slice::from_ref(&clause))
        .unwrap_or(clause)
}

/// Any Data Catalog operation whose `DatabaseName`/`Name` names no database
/// (`GetDatabase`, `DeleteDatabase`, `UpdateDatabase`, and the parent-database miss
/// of every table operation that names the database it could not resolve:
/// `GetTables`, `CreateTable`, `UpdateTable`, `DeleteTable`, `BatchDeleteTable`). The
/// catalog the emulator resolved is not part of the refusal.
///
/// RECORDED template: the one variable part is the database the request addressed, and
/// it is rendered as the catalog KEYS it rather than as the request spelled it — the
/// Hive-compatible fold every catalog name is stored and looked up under
/// ([`super::fold_name`]), which the model states for the member ("The name of the
/// database. For Hive compatibility, this is folded to lowercase when it is stored",
/// `DatabaseInput$Name`). So a mixed-case
/// request is refused in the canonical name, and a client matching the refusal against
/// the name the catalog holds matches.
pub fn database_not_found(name: &str) -> String {
    format!("Database {} not found.", super::fold_name(name))
}

/// `CreateDatabase` (and `UpdateDatabase`, whose rename collides the same way) for
/// a `DatabaseInput$Name` already taken. Recorded constant: the requested name is
/// deliberately withheld.
pub const DATABASE_ALREADY_EXISTS: &str = "Database already exists.";

/// A Data Catalog metadata read of a DATABASE the caller holds no Lake Formation
/// permission on ([`super::lake_formation`]), refused as unauthorized rather than
/// answered.
///
/// RECORDED template: the fixed text is "Insufficient Lake Formation permission(s):
/// Required Describe on " and the one variable part is the database the request
/// addressed, taken from the request. `Describe` is the Lake Formation permission a
/// metadata read needs, so it is the permission the sentence names.
pub fn insufficient_lake_formation_describe(database_name: &str) -> String {
    format!("Insufficient Lake Formation permission(s): Required Describe on {database_name}")
}

/// A Data Catalog metadata read of a TABLE the caller holds no Lake Formation permission on
/// ([`super::lake_formation`]), refused as unauthorized rather than answered.
///
/// RECORDED template, and the same sentence the database-level refusal
/// ([`insufficient_lake_formation_describe`]) is worded with: the fixed text is
/// "Insufficient Lake Formation permission(s): Required Describe on " and the one variable
/// part is the table the request addressed, taken from the request. `Describe` is the Lake
/// Formation permission a metadata read needs, so it is the permission the sentence names.
///
/// The recording is the behavioural unit this wording was taken from — the reader of a
/// governed catalog calling `GetTable` on a table it holds no grant on, filed as
/// `issues/.../athena-glue-lakeformation-s3-athena-does-not-enforce-lake-formation-permissions-ungranted.md`,
/// which states that refusal's fixed text and its one variable part. Kept as a function of
/// its own, because which RESOURCE a refusal names is the operation's to decide.
pub fn insufficient_lake_formation_describe_table(table_name: &str) -> String {
    insufficient_lake_formation_describe(table_name)
}

/// A read that addresses ONE catalog resource by its `CatalogId` (`GetCatalog`) for a
/// name the addressed account's Data Catalog holds no catalog under. Recorded
/// constant: the whole sentence is fixed text, and the id the request addressed is
/// deliberately withheld — like [`DATABASE_ALREADY_EXISTS`] and unlike
/// [`database_not_found`], whose recorded wording does name its resource.
pub const CATALOG_NOT_FOUND: &str = "Catalog not found.";

/// A request whose `CatalogId` selects the DATA CATALOG an entity lives in but is not
/// spelled the way such an id is ([`super::is_data_catalog_id`]): it carries no account-id
/// prefix, or characters no catalog id is written with.
///
/// Recorded constant: the whole sentence pair is fixed text. It states the rule — the
/// account-id prefix, and the characters an id may carry — and interpolates nothing: not
/// the rejected id, not an account, not a Region and not a name. Being a verdict on the
/// request rather than on the store, it is answered as the operation's modelled
/// `InvalidInputException` before the catalog is resolved, so it never depends on which
/// catalogs happen to exist ([`CATALOG_NOT_FOUND`] is the miss a WELL-FORMED id that names
/// no catalog earns).
pub const CATALOG_ID_INVALID: &str = "Catalog id is invalid. The catalog id must be \
     prepended with the account id prefix and can only contain lowercase letters, numbers, \
     underscores, and slashes.";

/// A `DeleteCatalog` whose `CatalogId` addresses the account's own Data Catalog — the
/// catalog every account HAS, which holds the catalogs a create registers and is not
/// one of them.
///
/// Recorded constant: that catalog is never deletable, and the refusal is the
/// operation's modelled `InvalidInputException` stating the rule a deletable catalog's
/// name satisfies, as whole fixed text — it interpolates nothing from the request or
/// from the store — rather than the miss a name holding no catalog earns
/// ([`CATALOG_NOT_FOUND`]).
pub const ROOT_CATALOG_NOT_DELETABLE: &str =
    "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name";

/// An `UpdateCatalog` whose `CatalogId` addresses a catalog whose databases live in a
/// source OUTSIDE the Glue Data Catalog — one stored with a `FederatedCatalog`
/// ([`super::CatalogDefinition::is_federated`]).
///
/// Such a catalog's definition is the federation's, not this catalog's own: the Data
/// Catalog holds the pointer at the source and the source holds everything under it, so
/// there is nothing here for a `CatalogInput` to write.
///
/// RECORDED constant, and the whole message: the service states that the write is not
/// allowed on a federated catalog — in the service's own spelling of that kind of catalog —
/// and interpolates neither the catalog the request addressed nor its source. The refusal is
/// the operation's modelled `InvalidInputException`.
pub const FEDERATED_CATALOG_NOT_UPDATABLE: &str = "Update on FederateCatalog is not allowed.";

/// An `UpdateCatalog` whose `CatalogId` addresses the account's own Data Catalog, which
/// holds the catalogs a create registers and is not one of them — so no `CatalogInput`
/// describes it.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the wording is the one the delete of that same catalog
/// answers ([`ROOT_CATALOG_NOT_DELETABLE`]), because both refuse the same catalog for the
/// same reason: the id addresses no catalog a write of this surface describes.
pub const ROOT_CATALOG_NOT_UPDATABLE: &str = ROOT_CATALOG_NOT_DELETABLE;

/// `CreateCatalog` for a `CatalogInput` that names no source outside the Glue Data
/// Catalog. Recorded constant: the whole sentence is fixed text, naming neither the
/// catalog the request asked for nor any member of its input.
pub const CREATE_NATIVE_CATALOG_UNSUPPORTED: &str =
    "Create glue native catalog is not supported.";

/// `CreateCatalog` for a federated catalog whose data source this account holds no
/// registration for ([`super::is_registered_federation_source`]). Recorded template:
/// the sentence names, in brackets, the data source the input pointed at and nothing
/// else of the request.
pub fn federation_source_not_registered(source: &str) -> String {
    format!("The data source [{source}] is not registered.")
}

/// `CreateCatalog` for a `Name` the account already holds a catalog under.
///
/// RECORDED constant, and the whole message: the sentence is fixed text that names the
/// resource kind, withholds the requested name and closes on a full stop — the sentence this
/// service words an already-exists condition with for every resource kind that has one
/// ([`DATABASE_ALREADY_EXISTS`], [`CONNECTION_ALREADY_EXISTS`], [`CRAWLER_ALREADY_EXISTS`],
/// [`REGISTRY_ALREADY_EXISTS`], [`TABLE_ALREADY_EXISTS`]), and the punctuation the catalog
/// surface's own miss sentence carries too ([`CATALOG_NOT_FOUND`]).
///
/// Two recordings of this one refusal disagree about that stop, and the stop is what this
/// constant states: it is the form the whole family of this service's already-exists
/// sentences takes, so a single missing character in one transcription of otherwise identical
/// fixed text is the likelier of the two to be wrong. The sentence is pinned per resource kind
/// by `tests/all/catalog_duplicate_create_wording.rs`, so a change to any of them fails a test.
pub const CATALOG_ALREADY_EXISTS: &str = "Catalog already exists.";

/// `CreateCatalog` whose `Name` is the account's own id. Stated by the model: the
/// catalog's name "Cannot be the same as the account ID" (`Catalog$Name`), because
/// that id already addresses the account's own Data Catalog. The sentence is that
/// requirement; what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException`.
pub const CATALOG_NAME_IS_ACCOUNT_ID: &str =
    "Catalog name cannot be the same as the account ID.";

/// `GetCatalogs` that asks for the default catalog while addressing a parent OTHER than the
/// account's own Data Catalog. Stated by the model: `IncludeRoot` governs "whether to list
/// the default catalog in the account and region", and "when the `ParentCatalogId` is not
/// equal to null, and this attribute is passed as `false` or `true`, an
/// `InvalidInputException` is thrown" — the default catalog is not inside any catalog, so
/// there is nothing for the flag to decide there. The sentence is that requirement; what is
/// pinned is the behaviour — 400 with the operation's modelled `InvalidInputException`.
pub const INCLUDE_ROOT_NEEDS_THE_ACCOUNT_CATALOG: &str =
    "IncludeRoot is not supported when ParentCatalogId names a catalog other than the \
     Amazon Web Services account ID.";

/// `CreateCatalog` whose `Name` carries a character the catalog surface forbids
/// ([`super::is_catalog_name`]).
///
/// RECORDED constant, and the whole message: the catalog states the character class a
/// name may be built from and nothing of the value it was handed, so one wording answers
/// every name outside it. It is NOT a constraint-violation report — the member path and
/// the expression the model declares are the boundary validator's words for a value
/// outside a `@pattern`, and this surface answers its own sentence instead.
pub const CATALOG_NAME_NOT_ALLOWED: &str =
    "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name";

/// The member a catalog-name constraint clause names: the catalog's own name, lower-camel
/// cased the way every clause of this envelope spells a member.
const CATALOG_NAME_MEMBER: &str = "name";

/// An operation that addresses a catalog resource by a `CatalogId` (`GetCatalog`,
/// `DeleteCatalog`, `UpdateCatalog`, and `GetCatalogs` by the parent it lists) whose id
/// carries a path segment that is no catalog NAME
/// ([`super::malformed_catalog_path_segment`]).
///
/// RECORDED template, and nothing of its wording is spelled here: the envelope is the
/// cross-service constraint-report one
/// ([`::service_common::validation_message::envelope_over`]) and the clause is the
/// framework's own rendering of a `@pattern` violation
/// (`aws_smithy_schema_server::Violation::message`), so this refusal reads exactly as one
/// the validating walk could have reported. Two fixed parts are decided here: the member
/// the clause names is the catalog's own `name` — the part of the id that failed, not the
/// wire member the id travelled in — and the requirement it states is
/// `CatalogNameString`'s expression as the model spells it
/// ([`super::CATALOG_NAME_PATTERN`]), anchors included, so a client that re-applies the
/// echoed expression matches what the service matched. The one variable part is the
/// rejected name, taken from the request.
pub fn catalog_name_outside_its_pattern(name: &str) -> String {
    let clause = ::aws_smithy_schema_server::Violation {
        path: CATALOG_NAME_MEMBER.to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::Pattern(
            super::CATALOG_NAME_PATTERN.to_string(),
        ),
        // The clause quotes the name the request addressed; a `CatalogId` carries no
        // `@sensitive`, so there is nothing to withhold.
        value: Some(name.to_string()),
        sensitive: false,
    }
    .message();
    // `envelope_over` answers `None` only for NO clause at all, and there is exactly one
    // here, so the fallback is unreachable; it is spelled rather than panicked on because a
    // refusal is not worth a panic, and the clause alone still states the requirement.
    ::service_common::validation_message::envelope_over(std::slice::from_ref(&clause))
        .unwrap_or(clause)
}

/// A READ that addresses ONE connection definition by name (`GetConnection`) for a
/// name the addressed Data Catalog holds none under. Recorded template: it names the
/// connection the request addressed and the account whose catalog was searched, both
/// read off the request — the name exactly as sent, and the catalog id the request
/// resolved to.
pub fn connection_not_found(name: &str, catalog_id: &str) -> String {
    format!("Connection with name {name} in account {catalog_id} is not found")
}

/// The `UpdateConnection` that addresses ONE connection definition by name for a name the
/// addressed Data Catalog holds none under. Recorded template of the update, and a
/// DIFFERENT sentence from the read's above — which is why the two are not one shared
/// string: it names the connection alone, built from the request, and no account.
pub fn connection_not_found_for_update(name: &str) -> String {
    format!("Connection not found: {name}")
}

/// The DELETE of a connection the addressed Data Catalog holds none of
/// (`DeleteConnection`).
///
/// Recorded CONSTANT, and the whole message: the delete states the resource kind it could
/// not resolve, with no trailing period, and echoes back neither the requested
/// `ConnectionName` nor the catalog it was addressed in — a THIRD wording for this one
/// miss, which is why the read's, the update's and the delete's are three strings and not
/// one.
pub const CONNECTION_NOT_FOUND_FOR_DELETE: &str = "Connection not found";

/// `CreateConnection` for a `ConnectionInput$Name` the addressed Data Catalog already
/// holds a connection under. Recorded constant, and the whole message: the service
/// states the resource kind and withholds the name the create asked for, as the
/// recorded `CreateDatabase` duplicate does for its own resource.
pub const CONNECTION_ALREADY_EXISTS: &str = "Connection already exists.";

/// `TestConnection` carrying an INLINE `TestConnectionInput` whose `ConnectionType` is one of
/// the LEGACY kinds ([`super::LEGACY_CONNECTION_TYPES`]) rather than a unified connection's.
///
/// RECORDED constant, and the whole message: its SUBJECT is the operation the restriction is
/// a restriction on, named as the API names it, and it states which connections that
/// operation supports; nothing else of the request reaches it — not the type it was handed,
/// not the properties it carries. It has no trailing period, as recorded. The refusal is a
/// 400 `ValidationException` — a class this operation's own `errors` list omits and the
/// SERVICE models, so the handler widens that shape in rather than inventing one.
///
/// The service words this one restriction with TWO sentences, one per request form, so this
/// constant answers the inline form only and
/// [`TEST_CONNECTION_STORED_UNIFIED_ONLY`] answers a stored connection addressed by name.
pub const TEST_CONNECTION_INPUT_UNIFIED_ONLY: &str =
    "TestConnection is only supported for the Unified Connections";

/// `TestConnection` addressing a STORED connection by `ConnectionName` whose
/// `ConnectionType` is one of the LEGACY kinds ([`super::LEGACY_CONNECTION_TYPES`]).
///
/// RECORDED constant, and the whole message: the same restriction as
/// [`TEST_CONNECTION_INPUT_UNIFIED_ONLY`], worded with the GENERIC subject rather than the
/// operation's name, and like it carrying nothing of the request — not the connection's name,
/// not its type — and no trailing period. The refusal is the same 400 `ValidationException`,
/// so the two forms are told apart by this sentence alone, which is why each form renders its
/// own and neither can be served by the other's.
pub const TEST_CONNECTION_STORED_UNIFIED_ONLY: &str =
    "This operation is only supported for the Unified Connections";

/// `TestConnection` whose inline `TestConnectionInput` states an `AuthenticationType`
/// without the member its credentials live in
/// ([`super::missing_authentication_member`]).
///
/// RECORDED template: the sentence is wrapped in the brackets this service states a list of
/// field-level validation failures in, and its two variable parts come from the request —
/// the member path, as AWS spells it from the `AuthenticationConfiguration` member down, and
/// the `AuthenticationType` that requires it. The refusal is the operation's modelled
/// `InvalidInputException`, a DIFFERENT class from the type-level refusal above, which is
/// what lets a caller tell a restriction on the kind of connection from a missing field.
pub fn authentication_member_required_for_type(
    member_path: &str,
    authentication_type: &str,
) -> String {
    format!(
        "[AuthenticationConfiguration.{member_path} is required for {authentication_type} \
         AuthenticationType.]"
    )
}

/// `TestConnection` that names neither the connection to test nor an inline definition to
/// test: both members are optional on their own, so a request carrying neither asks for no
/// connection at all.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the sentence follows the one this service states the same
/// either-or requirement with one surface over
/// ([`INTEGRATION_PROCESSING_PROPERTIES_REQUIRED`]), naming the two members in prose.
pub const TEST_CONNECTION_NAMES_NO_CONNECTION: &str =
    "Either ConnectionName or TestConnectionInput must be provided in the request.";

/// `TestConnection` that names a stored connection AND carries an inline definition: two
/// connections, where the call tests one. Stated by the model: "you can either provide an
/// existing connection name or a TestConnectionInput for testing a non-existing connection
/// input. Providing both at the same time will cause an error" (`TestConnection`).
///
/// NOT recorded: what is pinned is the behaviour the documentation states — 400 with the
/// operation's modelled `InvalidInputException` — in a sentence naming the two members,
/// beside the either-or above.
pub const TEST_CONNECTION_STATES_TWO_CONNECTIONS: &str =
    "ConnectionName and TestConnectionInput cannot both be provided in the same request.";

/// `TestConnection` for a connection definition this service accepted and then could not
/// connect through.
///
/// A unified connection names a data source OUTSIDE this emulator — a SaaS tenant, an
/// authorization server, a database host — and this emulator opens no connection to anything
/// outside itself, which is also why a stored connection carries no validation status at all
/// (`ConnectionEntry::from_input`). So the test reaches no data source, and what it reports
/// is that failure rather than a success nothing verified.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the sentence names the one thing the attempt was decided
/// by, the kind of data source the connection points at, taken from the definition under
/// test.
pub fn connection_test_did_not_reach_the_source(connection_type: &str) -> String {
    format!("Unable to connect to the {connection_type} data source.")
}

/// A read of the options of a connection TYPE (`DescribeConnectionType`) for a name that
/// names no connection type the service serves — neither one of the connector kinds it
/// ships ([`crate::state::connection_type_family`]) nor a custom one registered in this
/// account.
///
/// Recorded CONSTANT, reproduced as the service words this error class: the sentence states
/// that the connection type asked about is not available and names NOTHING of the request,
/// so a client sees the same wording whichever name it asked for.
pub const CONNECTION_TYPE_NOT_AVAILABLE: &str = "The connection type is not available.";

/// `RegisterConnectionType` whose `ConnectionType` does not carry the prefix the member
/// itself requires ("must be prefixed with "REST-" to indicate it is a REST-based
/// connector" — `RegisterConnectionTypeRequest$ConnectionType`).
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException`, and nothing registered — and the sentence states the member that
/// was validated and the prefix it must carry, the way this crate's other grammar refusals
/// of an identifier do. It names nothing of the
/// request, so every rejected name is refused with the same words.
pub fn connection_type_prefix_required(prefix: &str) -> String {
    format!("Invalid ConnectionType format. Valid connection types begin with {prefix}")
}

/// `RegisterConnectionType` for a `ConnectionType` this account has already registered a
/// custom connection type under.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException`, the closest client error it models (it models no
/// `AlreadyExistsException`), and the registration already stored left exactly as it was.
/// The sentence states the resource kind and withholds the name asked for, as the recorded
/// duplicate of a connection ([`CONNECTION_ALREADY_EXISTS`]) does for its own resource.
pub const CONNECTION_TYPE_ALREADY_EXISTS: &str = "Connection type already exists.";

/// `GetTable` for an address it cannot resolve — a table the database does not hold, or
/// a `DatabaseName` the catalog holds no database under. Recorded constant: this read
/// answers ONE wording whichever step of the address missed, and it names neither the
/// table nor the database, which is why the read does not share
/// [`database_not_found`] with the operations that address a database itself.
///
/// It is the TABLE READ's own sentence, not the catalog's refusal of every table
/// address: every other operation that resolves a table by name — the writes, the
/// version operations and the partition operations — answers the templated
/// [`table_not_found`] instead.
pub const TABLE_ENTITY_NOT_FOUND: &str = "Entity Not Found";

/// `DeleteTable` (and `UpdateTable`, the other write that addresses one table), every
/// operation over ONE table's version history (`GetTableVersion`, `GetTableVersions`,
/// `DeleteTableVersion` and `BatchDeleteTableVersion`, which names the table once for
/// the whole batch) and every partition operation's parent-table miss, for a table the
/// database does not hold. Recorded template naming no database — a different refusal
/// from `GetTable`'s, which is why the two are not one shared string.
///
/// Its one variable part is the table the request addressed, rendered in the same
/// Hive-compatible fold the catalog keys it under as [`database_not_found`] renders its
/// database — the model states it of this member too ("The table name. For Hive
/// compatibility, this is folded to lowercase when it is stored", `TableInput$Name`) —
/// so the refusal names the table the catalog holds, not the casing the request sent.
pub fn table_not_found(name: &str) -> String {
    format!("Table {} not found.", super::fold_name(name))
}

/// `CreateTable` for a table name the database already holds. NOT recorded either:
/// inferred from the recorded `CreateDatabase` duplicate, like
/// [`CRAWLER_ALREADY_EXISTS`].
pub const TABLE_ALREADY_EXISTS: &str = "Table already exists.";

/// A version read (`GetTableVersion`) or the single-version delete (`DeleteTableVersion`)
/// for a version id this table ONCE MINTED whose definition its history no longer holds.
///
/// Recorded CONSTANT, and the whole message: the refusal names the resource kind that
/// resolved to nothing and echoes back neither the `VersionId` the request addressed nor
/// the table it was addressed in. Which of this and [`VERSION_NOT_FOUND`] a miss earns is
/// decided by [`super::TableEntry::version_miss_message`], so neither of the two
/// single-version operations picks the wording itself. The BATCH delete reports the same
/// miss in band with its own templated sentence instead
/// ([`batch_table_version_not_found`]).
pub const TABLE_VERSION_NOT_FOUND: &str = "Table version not found.";

/// The same miss — a version id this table ONCE MINTED whose definition its history no
/// longer holds — as the BATCH delete reports it per item
/// (`TableVersionError$ErrorDetail$ErrorMessage`).
///
/// RECORDED template, and a different sentence from the single-version operations'
/// [`TABLE_VERSION_NOT_FOUND`]: the batch answers 200 and carries each failure in band, so
/// its per-item record STATES which version it is about rather than leaving that to the
/// exception the caller was handed. Its one variable part is the `VersionId` the item sent,
/// taken from the request and reported exactly as sent, like `TableVersionError$VersionId`
/// beside it.
///
/// Which of this and [`VERSION_NOT_FOUND`] a batch item earns is decided by
/// [`super::TableEntry::batch_version_miss_message`], over the same split
/// [`super::TableEntry::version_miss_message`] makes for the single-version operations, so
/// no operation picks the wording itself.
pub fn batch_table_version_not_found(version_id: &str) -> String {
    format!("Table version ({version_id}) not found")
}

/// The same two operations for an id that is NO version of the addressed table at all —
/// one past the counter its live definition carries, which the catalog has never minted
/// for it (a table whose history restarted under the same name is read this way).
///
/// Recorded CONSTANT, and the whole message: it states that the VERSION resolved to
/// nothing, and like [`TABLE_VERSION_NOT_FOUND`] carries no part of the request.
pub const VERSION_NOT_FOUND: &str = "Version not found.";

/// Any table-version operation whose `VersionId` spells no version counter
/// ([`super::parse_version_counter`]) — the version read's and the version delete's
/// one refusal for an id that is not a version id at all. NOT recorded: the sentence
/// restates the requirement the model declares for the member ("A VersionID is a
/// string representation of an integer", `GetTableVersionRequest$VersionId`) over the
/// value the request sent; what is pinned is the behaviour — 400 with the operation's
/// modelled `InvalidInputException`.
pub fn malformed_version_id(version_id: &str) -> String {
    format!("VersionId {version_id} is not a string representation of an integer")
}

/// `DeleteTableVersion` (and the per-version failure of `BatchDeleteTableVersion`) for
/// the version a table is CURRENTLY under. Recorded constant: the newest version of a
/// table is its live definition, which only `DeleteTable` removes, so the refusal
/// names no version and no table.
pub const TABLE_VERSION_IS_LATEST: &str = "VersionId cannot be equal to the latest.";

/// Any schema-registry operation whose `RegistryId` resolves to a registry this
/// endpoint holds none of — the parent-registry miss of a schema create, and the miss
/// of every operation that addresses a registry directly. Built from the resolved
/// registry name alone, like [`database_not_found`].
pub fn registry_not_found(name: &str) -> String {
    format!("Registry {name} not found.")
}

/// The schema READ (`GetSchema`) for a `SchemaId` whose registry this endpoint holds
/// none of: the registry is resolved before the schema set, so the refusal reports the
/// container that is missing rather than the schema inside it
/// ([`ComAmazonawsGlueAwsglueState::schema_or_miss`]).
///
/// RECORDED template: the fixed text is "Registry is not found. RegistryName: " and the
/// one variable part is the registry the `SchemaId` resolved to, taken from the request.
/// It is a THIRD wording for a registry miss — a different sentence from
/// [`registry_not_found`], which is what an operation addressing a registry by
/// `RegistryId` answers — which is why the two are not one string.
pub fn registry_not_found_for_schema(name: &str) -> String {
    format!("Registry is not found. RegistryName: {name}")
}

/// An operation of the integration API family that addresses ONE record by the ARN of
/// the resource it belongs to, for a `ResourceArn` this account holds no such record
/// for — the integration resource property read
/// (`GetIntegrationResourceProperty`), the resource miss of an integration table
/// properties write, and the miss of any of them whose `ResourceArn` addresses the
/// integration's TARGET — the Glue database it replicates into
/// ([`crate::service::integration_properties::table_properties_miss`]). RECORDED template: the sentence is fixed around its one variable
/// part, the ARN the request addressed, taken verbatim from `ResourceArn`; what is
/// pinned is the behaviour — `EntityNotFoundException`.
pub fn target_resource_not_found(resource_arn: &str) -> String {
    format!("The target resource {resource_arn} is not found.")
}

/// `CreateIntegrationResourceProperty` whose request states NEITHER
/// `SourceProcessingProperties` nor `TargetProcessingProperties`. Both members are
/// optional on their own, so this is the service's own requirement rather than a
/// modelled constraint. RECORDED constant: the whole sentence is fixed — it names the
/// two sides in prose and interpolates neither the addressed resource nor a member
/// path — and the refusal is the operation's modelled `ValidationException`.
pub const INTEGRATION_PROCESSING_PROPERTIES_REQUIRED: &str =
    "Either source processing properties or target processing properties must be provided \
         in the request.";

/// An operation that addresses ONE integration table property record
/// (`GetIntegrationTableProperties`, `DeleteIntegrationTableProperties`) for a pair this
/// account holds no record for, where the `ResourceArn` does NOT address the integration's
/// target; one that does earns [`target_resource_not_found`] instead
/// ([`crate::service::integration_properties::table_properties_miss`]). Recorded
/// template: the fixed text around the two
/// members the request addressed the record by, which the service names `sourceArn` and
/// `tableName`, and no trailing period.
pub fn integration_table_properties_not_found(resource_arn: &str, table_name: &str) -> String {
    format!(
        "Failed to find the sourceTableProperties with sourceArn {resource_arn} and tableName \
             {table_name}"
    )
}

/// A `CreateIntegrationTableProperties` (or its update) that states BOTH table
/// configurations in one call. Recorded constant, entirely fixed text: the model names
/// both members optional, and the service's rule is that one call carries the
/// configuration of the ONE leg its `ResourceArn` names — "the same API need to be
/// invoked with the Glue connection ARN as ResourceArn with SourceTableConfig, and the
/// Glue database ARN as ResourceArn with TargetTableConfig respectively".
pub const INTEGRATION_TABLE_CONFIG_SIDES_EXCLUSIVE: &str =
    "Source and Target config can't both be set.";

/// A write of a `SourceTableConfig` that states a member the service does not process
/// yet ([`super::UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS`]). Recorded constant,
/// entirely fixed text: it names neither the member nor the value that was refused.
pub const UNSUPPORTED_SOURCE_TABLE_PROPERTIES: &str =
    "Unsupported source table properties are provided.";

/// Any schema-registry operation whose `SchemaId` resolves to a schema set this
/// endpoint holds none of (`GetSchema`, `DeleteSchemaVersions`, and the parent-schema
/// miss of every version operation).
///
/// RECORDED constant, and the whole message: the service states the miss in FIXED
/// words and echoes back neither the schema the request addressed nor the registry it
/// was addressed in — the same shape as the recorded `GetTable` miss. Shared by every
/// schema lookup, so one operation's miss cannot drift from another's.
pub const SCHEMA_NOT_FOUND: &str = "Schema is not found.";

/// A schema-registry operation whose `SchemaId` resolves to a schema set this endpoint
/// holds none of, stated with the ADDRESS the request looked it up by
/// (`ListSchemaVersions`).
///
/// RECORDED template: the fixed sentence [`SCHEMA_NOT_FOUND`], then the three `SchemaId`
/// members as the request sent them, label by label and in this order — so a caller holding
/// several schema sets can tell which lookup missed. A member the request did not carry is
/// spelled `null`; the labels, their order and their spelling are fixed text. The same
/// address clause the version miss below carries, minus the two members a lookup that names
/// no version has nothing to say about.
pub fn schema_not_found_at(
    registry_name: Option<&str>,
    schema_name: Option<&str>,
    schema_arn: Option<&str>,
) -> String {
    /// How a member the request did not carry is spelled in the labels below.
    const ABSENT: &str = "null";
    format!(
        "{SCHEMA_NOT_FOUND} RegistryName: {}, SchemaName: {}, SchemaArn: {}",
        registry_name.unwrap_or(ABSENT),
        schema_name.unwrap_or(ABSENT),
        schema_arn.unwrap_or(ABSENT)
    )
}

/// A version read addressed by a `SchemaVersionId` no stored version carries
/// (`GetSchemaVersion`).
///
/// RECORDED constant, and the whole message: a `SchemaVersionId` is the WHOLE address of a
/// version — the request names no registry and no schema — so resolving one starts at the
/// schema the id belongs to, and it is THAT lookup the service reports as missing rather
/// than a version of a schema it never found. Nothing follows the sentence, which is why it
/// carries no trailing period, and it echoes back neither the id the request sent nor
/// anything else of the request.
pub const SCHEMA_NOT_FOUND_FOR_VERSION_ID: &str = "Schema is not found";

/// Any schema-registry operation whose request resolves a schema set but names NO version
/// of it in a way that carries an address: `GetSchemaByDefinition`, whose request addresses
/// a version by the definition stored on it rather than by a number, and the `UpdateSchema`
/// that asks for a checkpoint at a version the set does not hold.
///
/// NOT recorded: it states the miss in the fixed words of the recorded version miss below,
/// without the address labels, because such a request named no registry, schema or version
/// number for them to carry.
pub const SCHEMA_VERSION_NOT_FOUND: &str = "Schema version is not found.";

/// A version read whose `SchemaId` resolves a schema set this endpoint HOLDS, but whose
/// `SchemaVersionNumber` names no version of it (`GetSchemaVersion`).
///
/// RECORDED template: the fixed sentence [`SCHEMA_VERSION_NOT_FOUND`], then the ADDRESS the
/// request asked for, label by label and in this order — the three `SchemaId` members as the
/// request sent them, so the caller sees which form it addressed the schema set by, then the
/// `VersionNumber` it named and whether it asked for the latest version instead. A member the
/// request did not carry is spelled `null`; the labels, their order and their spelling
/// (`isLatestVersion` among the capitalised ones) are fixed text.
pub fn schema_version_not_found(
    registry_name: Option<&str>,
    schema_name: Option<&str>,
    schema_arn: Option<&str>,
    version_number: Option<i64>,
    latest_version: bool,
) -> String {
    /// How a member the request did not carry is spelled in the labels below.
    const ABSENT: &str = "null";
    let number = version_number.map_or_else(|| ABSENT.to_string(), |number| number.to_string());
    format!(
        "{SCHEMA_VERSION_NOT_FOUND} RegistryName: {}, SchemaName: {}, SchemaArn: {}, \
         VersionNumber: {number}, isLatestVersion: {latest_version}",
        registry_name.unwrap_or(ABSENT),
        schema_name.unwrap_or(ABSENT),
        schema_arn.unwrap_or(ABSENT)
    )
}

/// `RegisterSchemaVersion` against a schema set whose recorded `Compatibility` admits no
/// further version. Stated by `CreateSchema`: "Compatibility mode 'DISABLED' restricts any
/// additional schema versions from being added after the first schema version."
///
/// RECORDED template: the fixed sentence plus two variable parts — the mode recorded on
/// the schema set the request resolved to, and the `SchemaId` the request addressed it by,
/// rendered member for member ([`super::schema_id_description`]) because the mode is a
/// property of the schema set and the caller only sent the reference to it.
pub fn compatibility_does_not_allow_versioning(compatibility: &str, schema_id: &str) -> String {
    format!("Compatibility {compatibility} does not allow versioning. SchemaId: {schema_id}")
}

/// A `SchemaDefinition` the data format of the schema set it is being registered against
/// does not accept ([`super::schema_definition_fault`]).
///
/// RECORDED template: the fixed text "Schema definition of ", the `DataFormat` of the schema
/// set, " data format is invalid: ", and what the format's own parser reported about the
/// document — the position it stops being one at. Only the format and the parser's own
/// sentence are variable; the service's own parser is not the one that recorded the sentence,
/// so the wording of that part is this emulator's parser's.
pub fn schema_definition_invalid(data_format: &str, detail: &str) -> String {
    format!("Schema definition of {data_format} data format is invalid: {detail}")
}

/// `CreateSchema` for a `SchemaName` its registry already holds.
///
/// Recorded template: the fixed collision sentence followed by the MEMBER the create
/// addressed the schema set by, spelled as the model spells it, and the value it sent
/// — the only variable part. The registry the name was taken in is not disclosed.
pub fn schema_already_exists(schema_name: &str) -> String {
    format!("Schema already exists. SchemaName: {schema_name}")
}

/// A `PutSchemaVersionMetadata` or `RemoveSchemaVersionMetadata` whose `MetadataKeyValue`
/// does not carry both of its members.
///
/// NOT recorded: the model names both members of the wrapper optional, but a pair is
/// addressed by BOTH of them — "a metadata key" and "a metadata key's corresponding value"
/// (`MetadataKeyValuePair`) — so a wrapper naming one of them addresses no pair to put or
/// remove. Stated in the fixed words every schema-registry wrapper refusal uses
/// ([`super::SCHEMA_VERSION_REQUIRES_ID_OR_NUMBER`]), interpolating nothing.
pub const METADATA_PAIR_REQUIRES_BOTH: &str =
    "MetadataKey and MetadataValue have to be provided.";

/// `PutSchemaVersionMetadata` for a key value pair the addressed schema version already
/// holds.
///
/// NOT recorded: the sentence follows the template of the recorded create collision above
/// ([`schema_already_exists`]) — the fixed collision sentence, then the MEMBERS the request
/// addressed the pair by, spelled as the model spells them. Both are variable, because a
/// pair is addressed by both of them: putting another value under a key the version already
/// has is not a collision.
pub fn schema_version_metadata_already_exists(
    metadata_key: &str,
    metadata_value: &str,
) -> String {
    format!(
        "Metadata key value pair already exists. MetadataKey: {metadata_key}, MetadataValue: \
         {metadata_value}"
    )
}

/// `RemoveSchemaVersionMetadata` for a key value pair the addressed schema version does not
/// hold.
///
/// RECORDED template: the fixed sentence, then the ADDRESS the refusal reports it for — the
/// label `SchemaVersionId` and the id of the version the request resolved to, followed by the
/// separator the service renders after it whether or not anything follows. What is missing is
/// the metadata of that version, so the pair the request named is not echoed back.
pub fn schema_version_metadata_not_found(schema_version_id: &str) -> String {
    format!("Metadata is not found for the schema version. SchemaVersionId: {schema_version_id} ")
}

/// `PutSchemaVersionMetadata` against a schema version already holding as many metadata key
/// value pairs as one version may hold — the documented maximum of ten
/// ([`super::MAX_METADATA_PAIRS_PER_SCHEMA_VERSION`]).
///
/// RECORDED constant, and the whole message: the service states the limit as a resource limit
/// of the metadata surface in FIXED words, naming neither the number, the version nor the
/// pair that was refused.
pub const SCHEMA_VERSION_METADATA_LIMIT_EXCEEDED: &str =
    "Your resource limits for Schema Version Metadata have been exceeded.";

/// A column statistics task run lookup (`GetColumnStatisticsTaskRun`) for an id no
/// run in this account has. NOT recorded: the sentence follows the per-resource
/// template of the recorded crawler miss above, built from the requested id, which is
/// the whole address of a run.
pub fn column_statistics_task_run_not_found(run_id: &str) -> String {
    format!("Column statistics task run with id {run_id} does not exist")
}

/// A column of a stored table the catalog holds NO statistics for: the
/// `ErrorDetail$ErrorMessage` of that column's entry in a statistics read's `Errors`
/// ("a column name that failed and the reason for failure"), and the `Message` of the
/// miss an operation that addresses one single column answers with.
///
/// RECORDED constant for the READ's per-column entry, and the whole sentence: it
/// interpolates nothing — not the column, not the table, not the database — so the one
/// string is the wording wherever the catalog reports that a column has no statistics. The
/// column it is about travels in `ColumnError$ColumnName`, which is where the surface puts
/// it.
///
/// The single-column DELETE's own refusal is NOT recorded: it has no per-column channel to
/// report in, so it raises the modelled miss of an entity that does not exist, worded with
/// this same sentence rather than a second one invented for it.
pub const COLUMN_STATISTICS_NOT_FOUND: &str = "Statistics do not exist for this column";

/// One entry of a column statistics write (`UpdateColumnStatisticsForTable`) whose
/// `StatisticsData` does not carry the figures its own `Type` names: the
/// `ErrorDetail$ErrorMessage` of that entry's place in `Errors`.
///
/// NOT recorded: the sentence states the stated type and the `ColumnStatisticsData`
/// member that type's figures belong in — "only one data object should be set and
/// indicated by the `Type` attribute" — which is what the caller has to send for the
/// entry to be accepted. Both parts are read off the request.
pub fn column_statistics_data_member_missing(statistics_type: &str, member: &str) -> String {
    format!("ColumnStatisticsData of type {statistics_type} must carry {member}.")
}

/// A column statistics write for a column past the number of columns this store keeps
/// statistics for ([`super::MAX_COLUMN_STATISTICS_PER_TABLE`]): the
/// `ErrorDetail$ErrorMessage` of that entry's place in `Errors`.
///
/// NOT recorded, because the ceiling is this emulator's own capacity rather than an AWS
/// quota: the sentence follows the wording of the other capacity refusals of this crate and
/// states the ceiling that was reached.
pub fn column_statistics_limit_exceeded(limit: usize) -> String {
    format!("the table already holds the maximum of {limit} column statistics")
}

/// A column statistics task settings read (`GetColumnStatisticsTaskSettings`) for a
/// table no create has written settings for. RECORDED constant: the whole sentence is
/// fixed — it names the MEMBERS the request addressed the entry by rather than the
/// values it sent, so neither the database nor the table name appears in it.
pub const COLUMN_STATISTICS_TASK_SETTINGS_NOT_FOUND: &str =
    "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist";

/// The absence of the column statistics task settings a SCHEDULE operation addresses —
/// `StartColumnStatisticsTaskRunSchedule` and `StopColumnStatisticsTaskRunSchedule`, which
/// act on the schedule those settings carry and so report the SETTINGS record as the entity
/// that is missing, whatever the catalog holds under the names on the way to it.
///
/// RECORDED template: the fixed text is "ColumnStatisticsTaskSettings " and " not found.",
/// and the variable part between them is the record's own composite key — the catalog the
/// request addressed, the account whose estate that catalog is in, then the database and the
/// table the settings belong to, as the request named them. For the default catalog both id
/// components are the account's own id, because that id is what the catalog is addressed by
/// ([`super::resolve_catalog_id`]).
///
/// The settings READ, WRITE and DELETE word the same absence their own way
/// ([`COLUMN_STATISTICS_TASK_SETTINGS_NOT_FOUND`]): each operation's wording is recorded on
/// its own, which is why the two are two strings.
pub fn column_statistics_task_settings_key_not_found(
    catalog_id: &str,
    database_name: &str,
    table_name: &str,
) -> String {
    format!(
        "ColumnStatisticsTaskSettings {catalog_id}:{}:{database_name}-{table_name} not found.",
        ::engine::emulator_account_id()
    )
}

/// `StartColumnStatisticsTaskRunSchedule` or `StopColumnStatisticsTaskRunSchedule` for a
/// stored table whose column statistics task settings carry NO cron expression
/// ([`super::ColumnStatisticsTaskSettingsEntry::has_schedule`]): the entry exists, the
/// schedule those two operations act on does not.
///
/// The REFUSAL is the one the model documents for exactly this condition on the service's
/// other schedule pair: `StartCrawlerSchedule` declares `NoScheduleException`, "there is no
/// applicable schedule", for a record carrying no expression to run. These two operations
/// declare no such class (`smithy_query operation=StartColumnStatisticsTaskRunSchedule` lists
/// `AccessDeniedException`, `EntityNotFoundException`, `InvalidInputException` and
/// `OperationTimeoutException`), so the absent schedule is reported in the one class they do
/// declare for a thing that does not exist — and in the SENTENCE the service states when no
/// schedule was found ([`NO_SCHEDULE_FOUND`]), rather than a second one invented for this
/// surface.
pub const COLUMN_STATISTICS_TASK_RUN_SCHEDULE_NOT_FOUND: &str = NO_SCHEDULE_FOUND;

/// `CreateColumnStatisticsTaskSettings` addressed at a `TableName` the addressed
/// database holds no table under. RECORDED constant: the whole sentence is fixed — it
/// names the MEMBERS the request addressed the table by rather than the values it
/// sent, like this operation's other recorded refusals, so the requested table name is
/// withheld. A SEPARATE string from the catalog's own [`table_not_found`] template,
/// which is the recorded wording of the table writes (`DeleteTable`, `UpdateTable`).
pub const COLUMN_STATISTICS_TASK_SETTINGS_TABLE_NOT_FOUND: &str =
    "Table passed as input within input database not found.";

/// `CreateColumnStatisticsTaskSettings` for a table whose settings a create already
/// wrote.
///
/// RECORDED template: fixed text after the record the settings belong to, which the
/// surface addresses as the catalog that holds them, then the database and the table the
/// settings were written for — every part read off the request, nothing of this one run.
pub fn column_statistics_task_settings_already_exist(
    catalog_id: &str,
    database_name: &str,
    table_name: &str,
) -> String {
    format!("{catalog_id}:{database_name}-{table_name} already exists")
}

/// `StartColumnStatisticsTaskRun` for a table that already has a run in flight — the
/// operation's modelled `ColumnStatisticsTaskRunningException`, "thrown when you try
/// to start another job while running a column stats generation job". RECORDED
/// constant: the whole sentence is fixed, naming neither the database, the table nor
/// the in-flight run, so a client can recognise the transient conflict by the text
/// alone.
pub const COLUMN_STATISTICS_TASK_RUN_ALREADY_STARTED: &str =
    "Column Statistics task for input table has already started";

/// `CreateColumnStatisticsTaskSettings` while a statistics run of the addressed table
/// is in flight — the same modelled `ColumnStatisticsTaskRunningException`. RECORDED
/// constant, and a SEPARATE string from the start's above: the two recorded sentences
/// differ in the case of "statistics", so each operation answers with its own rather
/// than sharing one.
pub const COLUMN_STATISTICS_TASK_SETTINGS_RUN_ALREADY_STARTED: &str =
    "Column statistics task for input table has already started";

/// `StopColumnStatisticsTaskRun` for a table with no statistics run in flight — the
/// operation's modelled `ColumnStatisticsTaskNotRunningException`, "thrown when you
/// try to stop a task run when there is no task running".
///
/// RECORDED constant, and the whole message: the sentence states that no run of the
/// requested database and table stands in the state a stop acts on, naming the MEMBERS the
/// request addressed the run by rather than the values it sent — so neither the database nor
/// the table is disclosed, as in the in-flight refusals above.
pub const COLUMN_STATISTICS_TASK_NOT_RUNNING: &str =
    "No task found in Running state for input database and table name combination";

/// `CreateRegistry` for a `RegistryName` this account already holds. NOT recorded
/// either: inferred from the recorded `CreateDatabase` duplicate, like
/// [`CRAWLER_ALREADY_EXISTS`] — 400, `AlreadyExistsException`, and no disclosure of the
/// requested name.
pub const REGISTRY_ALREADY_EXISTS: &str = "Registry already exists.";

/// `CreateWorkflow` for a `Name` this account already holds a workflow under.
///
/// Recorded constant, and the whole message: the service states the collision over
/// the MEMBER the create named the workflow in rather than the value, so the
/// requested name is withheld — reproduced as recorded, its capital `E` and its
/// missing trailing period included, because a client recognises the refusal by its
/// wording.
pub const WORKFLOW_ALREADY_EXISTS: &str = "Workflow with given name already Exists";

/// `CreateBlueprint` for a `Name` this account already holds a blueprint under.
///
/// Recorded constant, and the whole message: like [`WORKFLOW_ALREADY_EXISTS`] it
/// names the member the create addressed the blueprint by and withholds the value,
/// with no trailing period, as recorded.
pub const BLUEPRINT_ALREADY_EXISTS: &str = "Blueprint with given name already exists";

/// Any blueprint operation whose `Name` names no blueprint this account holds
/// (`GetBlueprint`, and the same miss on every sibling that addresses one by name).
///
/// RECORDED constant, and the whole message: like [`BLUEPRINT_ALREADY_EXISTS`] it names
/// the member the request addressed the blueprint by and withholds the value, so one name
/// cannot be disclosed by a miss on another, and every operation that addresses a
/// blueprint by name reports the miss with this one sentence.
pub const BLUEPRINT_NOT_FOUND: &str = "Blueprint with given name not found.";

/// A blueprint operation whose blueprint has no ACTIVE definition to work from — a run
/// started of a registration that has not finished, or of one that finished without a
/// definition ([`super::BlueprintEntry::is_active`]).
///
/// RECORDED template: the sentence is fixed and its one variable part is the blueprint
/// the request named, taken from the request's own `BlueprintName`. The start is refused
/// with the operation's modelled `IllegalBlueprintStateException`, "the blueprint is in
/// an invalid state to perform a requested operation".
pub fn blueprint_has_no_active_definition(name: &str) -> String {
    format!("No active definition found for Blueprint: {name}")
}

/// `Blueprint$ErrorMessage` for a registration whose published archive could not be read:
/// "An error message."
///
/// RECORDED template: the fixed text "Access denied on object at location: " followed by the
/// `BlueprintLocation` the registration named, taken verbatim from the request, and a
/// terminating period. Only the location is variable; the rest is the service's own wording
/// for an archive it could not fetch.
pub fn blueprint_location_not_accessible(location: &str) -> String {
    format!("Access denied on object at location: {location}.")
}

/// A blueprint run that could not create the workflow it lays out because this account
/// already holds a workflow of that name: `BlueprintRun$ErrorMessage`, "indicates any
/// errors that are seen while running the blueprint". NOT recorded: what is pinned is the
/// behaviour — the run reaches the modelled failed state having created nothing — and the
/// sentence names the workflow the run would have created, which is the one thing a
/// caller needs in order to act on it.
pub fn blueprint_run_workflow_exists(workflow_name: &str) -> String {
    format!(
        "The blueprint run could not create workflow {workflow_name}: a workflow of that \
             name already exists."
    )
}

/// A blueprint run whose layout ran and created no entity at all:
/// `BlueprintRun$ErrorMessage`, "indicates any errors that are seen while running the
/// blueprint". NOT recorded: what is pinned is the behaviour — a run whose layout produces no
/// entity reaches the modelled failed state rather than the succeeded one, because there is no
/// workflow for a caller to run — and the sentence states exactly that.
pub const BLUEPRINT_RUN_DEPLOYED_NOTHING: &str =
    "The blueprint's layout created no job, crawler or trigger, so the blueprint run deployed \
     no workflow.";

/// A blueprint run lookup (`GetBlueprintRun`) that resolves no run at the address it was
/// given — a `RunId` the named blueprint holds no run under, and a `BlueprintName` this
/// account holds no blueprint under at all.
///
/// RECORDED template: the fixed text is "Blueprint run with id:", " and blueprint name:"
/// and " not found", and the two variable parts are the `RunId` and the `BlueprintName` the
/// request itself sent, which together are the whole address of a blueprint run. The read
/// addresses the RUN, so BOTH halves of a miss are reported in these words rather than the
/// blueprint surface's own sentence ([`BLUEPRINT_NOT_FOUND`]) — the recorded miss answers
/// this sentence for an address whose blueprint this account holds nothing under either, the
/// same way the workflow surface answers a workflow miss and a run miss alike
/// ([`WORKFLOW_NOT_FOUND`]).
pub fn blueprint_run_not_found(blueprint_name: &str, run_id: &str) -> String {
    format!("Blueprint run with id:{run_id} and blueprint name:{blueprint_name} not found")
}

/// A blueprint run HISTORY read (`GetBlueprintRuns`) for a `BlueprintName` this account
/// holds no blueprint under.
///
/// RECORDED constant, and the whole message: the history read answers with the service's
/// generic absent-entity sentence — the same text [`WORKFLOW_NOT_FOUND`] carries — naming
/// neither the blueprint nor the member it was addressed by, where the single-run read states
/// the run's whole address ([`blueprint_run_not_found`]) and the blueprint surface names the
/// member it was addressed by ([`BLUEPRINT_NOT_FOUND`]): each operation's wording is its own.
pub const BLUEPRINT_RUNS_NOT_FOUND: &str = "Entity not found";

/// The miss of the WORKFLOW surface: a `Name` that names no workflow this account holds
/// (`GetWorkflow`, and every sibling that addresses a workflow the same way), and a `RunId`
/// that names no run of a workflow that WAS found (`GetWorkflowRunProperties`,
/// `GetWorkflowRun`, `PutWorkflowRunProperties`), as well as the workflow a trigger's
/// definition names but nothing holds.
///
/// RECORDED constant, and the whole message: the sentence is the fixed, generic one this
/// surface answers a miss with, with no trailing period and naming neither the workflow nor
/// the run — so neither half of a run's address is disclosed, and the same text answers the
/// workflow miss and the run miss alike.
pub const WORKFLOW_NOT_FOUND: &str = "Entity not found";

/// A run-history read (`GetWorkflowRuns`) of a workflow this account DOES hold whose
/// history has no run to report. `GetWorkflowRunsResponse$Runs` is `length 1..1000`, so
/// a success carrying nothing is unrepresentable and the read is refused with the
/// operation's modelled `EntityNotFoundException` instead. NOT recorded: the sentence is
/// the fixed text Glue answers a run-history read with when there is no run history —
/// it names no resource, because the entity that is missing is the run history itself
/// and not the workflow, which was found.
pub fn no_workflow_runs_found() -> String {
    "No workflow runs were found.".to_string()
}

/// `StartWorkflowRun` for a workflow no trigger can begin a run of. RECORDED
/// template: the sentence is fixed and its one variable part is the workflow the
/// request named, taken from the request's own `Name`. The start is refused with the
/// operation's modelled `InvalidInputException` — a workflow with no entry point is
/// not something the service can run.
pub fn no_starting_trigger(name: &str) -> String {
    format!("Workflow {name} doesn't have any starting trigger.")
}

/// `StartJobRun` for a job that already has as many runs in flight as its
/// `ExecutionProperty$MaxConcurrentRuns` admits.
///
/// RECORDED constant, and the whole message: the job-run concurrency refusal is FIXED text
/// with no variable part — it names neither the job, nor how many runs are in flight, nor
/// the ceiling — so a client recognises the refusal by its wording whatever it started. The
/// WORKFLOW surface words its own concurrency refusal differently
/// ([`concurrent_workflow_runs_exceeded`]), which is why this is a second constant and not
/// that one.
pub const CONCURRENT_JOB_RUNS_EXCEEDED: &str = "Concurrent runs exceeded.";

/// `StartWorkflowRun` for a workflow already running as many runs as it allows.
/// DOCUMENTED template: `WorkflowRun$ErrorMessage` states this is the wording — "This
/// error message describes any error that may have occurred in starting the workflow
/// run. Currently the only error message is "Concurrent runs exceeded for workflow:
/// `foo`."" — so the sentence is fixed and its one variable part is the workflow the
/// request named. The start is refused with the operation's modelled
/// `ConcurrentRunsExceededException`.
pub fn concurrent_workflow_runs_exceeded(name: &str) -> String {
    format!("Concurrent runs exceeded for workflow: {name}.")
}

/// A data quality rule recommendation run lookup
/// (`GetDataQualityRuleRecommendationRun`, `CancelDataQualityRuleRecommendationRun`) for an
/// id no run in this account has.
///
/// RECORDED template, built through [`data_quality_not_found`]: a recommendation run is one
/// of the service's DATA QUALITY RUNS, which is the resource class the sentence names.
pub fn data_quality_rule_recommendation_run_not_found(run_id: &str) -> String {
    data_quality_not_found(DATA_QUALITY_RUN, run_id)
}

/// The resource class a data quality RUN miss names — both a rule recommendation run and a
/// ruleset evaluation run, which are the two kinds of run this surface starts, so one miss
/// cannot read one way for the first and another way for the second. It says nothing about
/// either id's GRAMMAR, which is stated by the operations that address a run
/// (`crate::state::DATA_QUALITY_RUN_ID_PREFIX`, applied where a recording shows it).
const DATA_QUALITY_RUN: &str = "Data Quality Run";

/// The resource class a data quality RESULT miss names.
const DATA_QUALITY_RESULT: &str = "Data Quality Result";

/// The sentence every data quality miss is worded with: the resource class the request
/// addressed, the account whose namespace holds no such record, and the id the request
/// itself sent, which is the whole address of one.
///
/// RECORDED template, one wording, so a miss cannot read one way for a run and another for
/// a result. It is the same SHAPE a machine learning transform miss takes
/// ([`ml_transform_not_found`]) and not the same sentence: that one names the handle a
/// transform is addressed by (`… with handle <id>`) where this one names an id, so the two
/// are kept apart rather than folded into a template neither recording shows.
fn data_quality_not_found(resource: &str, id: &str) -> String {
    format!(
        "Cannot find {resource} in account {} with id {id}",
        ::engine::emulator_account_id()
    )
}

/// A `RunId` that is no data quality run id at all — on any operation that addresses one of
/// the service's data quality runs by id, a rule recommendation run
/// (`GetDataQualityRuleRecommendationRun`, `CancelDataQualityRuleRecommendationRun`) or a
/// ruleset evaluation run (`GetDataQualityRulesetEvaluationRun`,
/// `BatchGetDataQualityRulesetEvaluationRun`) alike.
///
/// RECORDED constant apart from the grammar it quotes, which is the one prefix the store
/// MINTS a run id with ([`crate::state::DATA_QUALITY_RUN_ID_PREFIX`]) rather than a second
/// spelling of it — so the stated grammar and the minted one cannot disagree. It echoes
/// nothing of the request, so one wording answers every value outside the grammar.
pub fn invalid_data_quality_run_id() -> String {
    format!(
        "Invalid RunId: must start with '{}'",
        crate::state::DATA_QUALITY_RUN_ID_PREFIX
    )
}

/// A `ProfileId` that is no data quality profile id at all — one outside the grammar the
/// service mints a profile id with ([`super::is_data_quality_profile_id`]) — on any of the
/// three operations of the profile surface.
///
/// A value outside that grammar names no profile that could ever exist, so it is a malformed
/// request rather than a miss: RECORDED behaviour is the operations' modelled
/// `InvalidInputException`, where a well-formed id nothing minted is the
/// `EntityNotFoundException` above. RECORDED template: two fixed sentences, the second
/// quoting the one prefix the store MINTS a profile id with
/// ([`crate::state::DATA_QUALITY_PROFILE_ID_PREFIX`]) rather than a second spelling of it, so
/// the stated grammar and the minted one cannot disagree. It echoes nothing of the request,
/// so one wording answers every value outside the grammar.
pub fn invalid_data_quality_profile_id() -> String {
    invalid_data_quality_identifier("profileId", crate::state::DATA_QUALITY_PROFILE_ID_PREFIX)
}

/// A `StatisticId` that is no data quality statistic id at all — one outside the grammar the
/// service mints a statistic id with ([`super::is_data_quality_statistic_id`]) — on an
/// operation that addresses a statistic.
///
/// The SAME sentence class as the profile id's above, over the other minted id of this
/// surface: the refusal names the member that was validated and the prefix that member's
/// identifiers begin with, so a caller is told which of the request's ids is malformed
/// instead of a constraint on a member it sent correctly.
pub fn invalid_data_quality_statistic_id() -> String {
    invalid_data_quality_identifier(
        "statisticId",
        crate::state::DATA_QUALITY_STATISTIC_ID_PREFIX,
    )
}

/// The one sentence this surface refuses a malformed identifier with, over the member it
/// validated and that member's identifier prefix.
///
/// RECORDED template: two fixed sentences, the second quoting the prefix the store MINTS that
/// id with rather than a second spelling of it, so the stated grammar and the minted one
/// cannot disagree. The member name is the one the request spells, so the refusal describes
/// the constraint on the value that failed it and on no other member. It echoes nothing of the
/// request, so one wording answers every value outside the grammar.
fn invalid_data_quality_identifier(member: &str, prefix: &str) -> String {
    format!("Invalid {member} format. Valid identifiers begin with {prefix}")
}

/// A machine learning transform lookup (`GetMLTransform`, `UpdateMLTransform`,
/// `DeleteMLTransform`, or a walk over one transform's task runs) for a `TransformId` no
/// transform in this account has.
///
/// RECORDED template: the sentence is fixed and its two variable parts are the account
/// whose namespace holds no such transform and the `TransformId` the request itself
/// sent, which is the whole address of a transform — a `Name` is not, because "Names are
/// not guaranteed unique and can be changed at any time" (`MLTransform$Name`). Every
/// operation that misses a transform refuses it in THESE words, so one miss cannot be
/// worded one way by a read and another by a write.
pub fn ml_transform_not_found(transform_id: &str) -> String {
    format!(
        "Cannot find MLTransform in account {} with handle {transform_id}",
        ::engine::emulator_account_id()
    )
}

/// A machine learning task run lookup (`GetMLTaskRun`) for a `TaskRunId` the addressed
/// transform holds no run under, and the `CancelMLTaskRun` that addresses one the same way.
///
/// RECORDED template: the fixed text is `No ML Task Run found for `, `: in account ` and
/// ` for transform ` — the colon stands directly after the task run id — and the variable
/// parts are the `TaskRunId` the request sent, the account whose namespace was searched and
/// the `TransformId` the run was addressed under. A DIFFERENT sentence from the parent
/// transform's miss ([`ml_transform_not_found`]), which names only the transform, because
/// this one is about the run and names the whole of a run's address.
pub fn ml_task_run_not_found(task_run_id: &str, transform_id: &str) -> String {
    format!(
        "No ML Task Run found for {task_run_id}: in account {} for transform {transform_id}",
        ::engine::emulator_account_id()
    )
}

/// The label file path a label task run names (`StartImportLabelsTaskRun$InputS3Path`,
/// `StartExportLabelsTaskRun$OutputS3Path`) when it is no Amazon S3 path at all: the
/// labels travel in an object, so a value naming no bucket names nowhere to read them from
/// or write them to.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException`, no task run started — and the message states the member the
/// request filled in and the value it filled it with, as this service's other
/// input-validation refusals do. WHERE the object store could not serve an otherwise
/// well-formed path is a condition on the RUN, reported as the status that run comes to rest
/// in rather than as a refusal of the start.
pub fn invalid_label_file_path(member: &str, path: &str) -> String {
    format!("{member} {path} is not a valid Amazon S3 path of the form s3://bucket/key")
}

/// `CancelMLTaskRun` against a task run whose task the service is no longer working
/// through ([`super::MlTaskRunEntry::is_cancellable`]): a run that has already stopped,
/// finished or been terminated, and one already stopping, has no task left for a cancel
/// to stop.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the wording follows the one this service already states
/// the same rule in for a run of a JOB ([`JOB_RUN_CANNOT_BE_STOPPED`]), one resource kind
/// over, which states neither the status the run stands in nor the ids the request
/// addressed it by.
pub const ML_TASK_RUN_CANNOT_BE_CANCELLED: &str =
    "Task run cannot be cancelled in current state.";

/// `CreateMLTransform` for a `Name` this account already holds a transform under.
///
/// RECORDED template: the sentence is fixed and its one variable part is the `Name` the
/// request itself sent, which is the name the create asked to take. The create is
/// refused with the operation's modelled `AlreadyExistsException`, "a resource to be
/// created or added already exists".
pub fn ml_transform_already_exists(name: &str) -> String {
    format!("Transform with name {name} already exists")
}

/// `StartDataQualityRuleRecommendationRun` for a data source whose recommendation run
/// has not finished. NOT recorded: what is pinned is the behaviour — 400 with the
/// operation's modelled `ConflictException` — and the sentence names the table the
/// request addressed, the way the recorded per-resource templates name their
/// resource.
pub fn data_quality_run_already_running(database_name: &str, table_name: &str) -> String {
    format!(
        "A data quality rule recommendation run is already in progress for table \
             {database_name}.{table_name}"
    )
}

/// `StartDataQualityRuleRecommendationRun` whose `ClientToken` an earlier start already
/// used. RECORDED template: the sentence is fixed and carries the `ClientToken` the
/// request itself sent. The start is refused with the operation's modelled
/// `ConflictException` — a token stands for ONE start, so the service answers a second
/// one carrying it as the duplicate it is rather than starting another run.
pub fn idempotent_start_with_token(client_token: &str) -> String {
    format!("Idempotent start Data Quality run with token {client_token}")
}

/// A data quality ruleset evaluation run lookup
/// (`GetDataQualityRulesetEvaluationRun`) for an id no run in this account has.
///
/// The same recorded template as [`data_quality_rule_recommendation_run_not_found`] and the
/// same resource class: both are data quality RUNS of this surface, so one miss cannot be
/// worded one way by a recommendation run's read and another by an evaluation run's.
pub fn data_quality_ruleset_evaluation_run_not_found(run_id: &str) -> String {
    data_quality_not_found(DATA_QUALITY_RUN, run_id)
}

/// A data quality RESULT lookup (`GetDataQualityResult`) for an id no result in this
/// account has.
///
/// RECORDED template, built through [`data_quality_not_found`].
pub fn data_quality_result_not_found(result_id: &str) -> String {
    data_quality_not_found(DATA_QUALITY_RESULT, result_id)
}

/// The two reads of the anomaly-detection model (`GetDataQualityModel`,
/// `GetDataQualityModelResult`) for an address they can resolve no model result at: a
/// `ProfileId` no statistics run of this account minted, a `StatisticId` the addressed
/// profile holds no datapoint of, and — for the read of the result itself — a model whose
/// training has produced nothing yet.
///
/// RECORDED constant, and the whole message: the model reads state that the result was not
/// found and echo back neither the profile nor the statistic the request addressed, so one
/// wording answers every address that resolves to no model result. The refusal is the
/// operations' modelled `EntityNotFoundException`.
pub const DATA_QUALITY_MODEL_RESULT_NOT_FOUND: &str = "Unable to find Model Result";

/// `PutDataQualityProfileAnnotation` for a `ProfileId` no statistics run of this account
/// minted.
///
/// RECORDED template: unlike the model reads above, the annotation write NAMES the profile it
/// could not resolve — the account whose namespace holds no such profile and the `ProfileId`
/// the request itself sent, which is the shape this service words a missing machine learning
/// run in too ([`ml_task_run_not_found`]) — and the refusal is the operation's modelled
/// `EntityNotFoundException`.
pub fn data_quality_profile_not_found(profile_id: &str) -> String {
    format!(
        "Cannot find Data Profile in account {} with id {profile_id}",
        ::engine::emulator_account_id()
    )
}

/// `GetDataQualityModelResponse$FailureReason` of a training that produced no model because
/// the profile it was asked for holds no datapoint at all.
///
/// NOT recorded: a model is trained over a statistic's datapoints, so a profile carrying none
/// leaves a training that produced nothing rather than one still running or one that
/// succeeded. Whole fixed text, and inside the member's `length 1..255`: the profile it is
/// about is the one the request addressed.
pub const DATA_QUALITY_MODEL_HAS_NO_STATISTICS: &str =
    "The profile holds no statistics to train a model on.";

/// A data quality run start whose `DataSource` carries neither kind of Glue table. NOT
/// recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the sentence names the members the input has to carry
/// one of, the way [`PARTITION_VALUES_REQUIRED`] names its member.
pub const DATA_SOURCE_REQUIRES_A_TABLE: &str =
    "DataSource must specify either a GlueTable or a DataQualityGlueTable.";

/// `DataQualityRulesetEvaluationRun$ErrorString` of a run whose data source could not be
/// read, so none of its rules was evaluated against any data.
///
/// NOT recorded: AWS evaluates the rules against the objects behind the table, so a run
/// whose data this emulator could not read reports that nothing was evaluated rather than a
/// score nothing computed. Whole fixed text — the table it is about is the data source the
/// run was started for, which the run already reports; WHY the data could not be read is
/// logged against the run.
pub fn data_quality_data_source_unreadable() -> String {
    "The data source of this run could not be read, so no rule was evaluated.".to_string()
}

/// `DataQualityRulesetEvaluationRun$ErrorString` of a run carrying a rule whose truth could
/// not be decided from the data source's rows.
///
/// NOT recorded, and the same choice [`data_quality_data_source_unreadable`] makes: a rule
/// nothing evaluated is reported as evaluated by nothing rather than as a pass or a fail.
/// The variable part is the rule, as its author wrote it, so a client can tell which rule of
/// its ruleset was not evaluated.
pub fn data_quality_rule_not_evaluated(rule: &str) -> String {
    format!("The rule {rule} was not evaluated, so this run produced no result.")
}

/// `DataQualityRulesetEvaluationRun$ErrorString` of a run whose rules over its rows were
/// more than one run carries out ([`super::MAX_DATA_QUALITY_RULE_CHECKS`]).
///
/// NOT recorded, and the same choice the two reasons above make: the run reports that it
/// evaluated nothing rather than a score over part of its rules. Whole fixed text — the
/// bound is this emulator's own capacity and no figure of the request.
pub fn data_quality_too_much_to_evaluate() -> String {
    "This run had more rules and rows to evaluate than one run evaluates, so it produced no \
     result."
        .to_string()
}

/// A data quality ruleset lookup for a name this account holds no ruleset under — the
/// miss of every operation that addresses one by `Name` (`GetDataQualityRuleset`, and
/// the update and delete that address a ruleset the same way), and of the evaluation
/// start that names the rulesets to evaluate in its `RulesetNames`.
///
/// Recorded template: the sentence names the account whose namespace was searched and
/// then the ruleset the request addressed, because a ruleset's name is "a unique name
/// for the data quality ruleset" per account and so the account is part of the address
/// that resolved to nothing. The name is read off the request, exactly as sent; the
/// account is the one this emulator serves, not a value the request carries.
pub fn data_quality_ruleset_not_found(name: &str) -> String {
    format!(
        "Cannot find Data Quality Ruleset in account {} with name {name}",
        ::engine::emulator_account_id()
    )
}

/// `CreateDataQualityRuleset` for a name this account already holds a ruleset under.
///
/// Recorded template: the fixed collision sentence plus the `Name` the create asked
/// for, its one variable part, read off the request exactly as sent.
pub fn data_quality_ruleset_already_exists(name: &str) -> String {
    format!("Another ruleset with the same name already exists: {name}")
}

/// A partition create whose `PartitionInput` carries no `Values`. NOT recorded: what
/// is pinned is the behaviour — `InvalidInputException`, as a per-item error for a
/// batch create — and the sentence names the member the input has to carry, which is
/// what the model requires of a valid input (`PartitionInput$Values`).
pub const PARTITION_VALUES_REQUIRED: &str = "PartitionInput must specify Values.";

/// A partition create whose value count is not the addressed table's partition-key
/// count, in EITHER direction (fewer values than keys, or more).
///
/// RECORDED constant, and the whole message: the sentence is fixed and has no variable
/// part at all — it names neither the table, nor the values, nor the two counts that
/// disagree, so one arity refusal reads the same as every other and a client matching the
/// text recognises the condition whatever was sent.
pub const PARTITION_KEYS_DO_NOT_MATCH_VALUES: &str =
    "The number of partition keys do not match the number of partition values";

/// A partition create addressed at a table that declares NO partition keys at all, so no
/// value list names a slice of it.
///
/// RECORDED constant, and the whole message: the sentence is fixed and names neither the
/// table nor the values the request sent. It is a DIFFERENT sentence from the arity refusal
/// a partitioned table answers ([`PARTITION_KEYS_DO_NOT_MATCH_VALUES`]): a table with no
/// partition keys holds no partitions whatever is sent, while a partitioned table refuses
/// only the lists whose length is not its key count, and the service words the two
/// conditions apart.
pub const PARTITIONS_IN_UNPARTITIONED_TABLE: &str =
    "Cannot create partitions in table without partition keys";

/// A partition create for a value list the addressed table already holds a partition
/// for. NOT recorded: inferred from the recorded `CreateDatabase` duplicate, like
/// [`TABLE_ALREADY_EXISTS`] — `AlreadyExistsException`, and no disclosure of the
/// requested values.
pub const PARTITION_ALREADY_EXISTS: &str = "Partition already exists.";

/// A partition update (`UpdatePartition`, one entry of `BatchUpdatePartition`) whose
/// `PartitionInput` names values other than the partition the request addresses. NOT
/// recorded: what is pinned is the behaviour — `InvalidInputException`, per entry for a
/// batch update — and the sentence states the documented rule the entry broke, that an
/// update writes a partition's definition and never moves it to another address
/// (`UpdatePartitionRequest$PartitionInput`). No stored value is disclosed.
pub const PARTITION_VALUES_CANNOT_BE_CHANGED: &str =
    "PartitionInput must specify the values of the partition being updated; the Values \
         property can't be changed.";

/// A partition read (`GetPartition`) for a value list the addressed table holds no
/// partition for, and a delete (one item of `BatchDeletePartition`) of one, which has
/// none to remove. RECORDED
/// constant: the whole sentence is fixed — it names neither the values the request
/// addressed nor the table they were looked for in.
pub const PARTITION_NOT_FOUND: &str = "Cannot find partition.";

/// A partition UPDATE (`UpdatePartition`, one entry of `BatchUpdatePartition`) of a value
/// list the addressed table holds no partition for, so there is nothing to write to.
///
/// RECORDED template, and a DIFFERENT sentence from the read's
/// ([`PARTITION_NOT_FOUND`]): the mutation names the partition it could not find, while the
/// read names none. The fixed parts are `Partition [` and `] not found` — no trailing full
/// stop — around the values the request addressed the partition by, in the order it listed
/// them. What is RECORDED is the single-value form (`Partition [2023] not found`); the
/// separator between two of them is CHOSEN, the one a list of values is written with, and is
/// the only part of this template no recording fixes.
pub fn partition_to_update_not_found(values: &[String]) -> String {
    format!("Partition [{}] not found", values.join(", "))
}

/// `CreateSecurityConfiguration` for a name already taken. NOT recorded: inferred
/// from the recorded `CreateDatabase` duplicate, like [`CRAWLER_ALREADY_EXISTS`] —
/// 400, `AlreadyExistsException`, and no disclosure of the requested name.
pub const SECURITY_CONFIGURATION_ALREADY_EXISTS: &str =
    "Security configuration already exists.";

/// A resource-policy read or delete (`GetResourcePolicy`, `DeleteResourcePolicy`) for
/// a resource that carries no policy. RECORDED constant: the refusal names neither
/// the resource nor the ARN that was addressed.
pub const POLICY_NOT_FOUND: &str = "Policy not found";

/// `PutResourcePolicy` carrying `PolicyExistsCondition` `MUST_EXIST` for a resource
/// that carries no policy yet ("A value of MUST_EXIST is used to update a policy"), and
/// any resource-policy write or delete whose `PolicyHashCondition` is not the hash of
/// the policy currently attached — the concurrent-modification check that member exists
/// for ("its purpose is to prevent concurrent modifications of a policy"), which a
/// resource carrying no policy at all fails too. RECORDED constant, and it names both
/// cases: the policy is absent, or the hash is not the attached policy's. No stored
/// value is disclosed.
pub const POLICY_CONDITION_FAILED: &str = "Policy not exists or policy hash mismatch";

/// `PutResourcePolicy` carrying `PolicyExistsCondition` `NOT_EXIST` against a resource
/// that already carries one ("A value of NOT_EXIST is used to create a new policy").
/// NOT recorded — the recorded condition failure above states the opposite case — so
/// what is pinned is the behaviour, 400 with the operation's modelled
/// `ConditionCheckFailureException`, and the sentence follows that wording and
/// discloses nothing of the attached policy.
pub const POLICY_ALREADY_EXISTS: &str = "Policy already exists";

/// A resource-policy request whose `ResourceArn` this endpoint can hold no policy for.
/// Derived from the request, like [`super::invalid_resource_arn_message`]: it names the
/// member and the value that is not a Glue resource of this account and Region.
pub fn invalid_policy_resource_arn(resource_arn: &str) -> String {
    format!(
        "ResourceArn must be the ARN of a Glue resource of this account and Region; got \
             {resource_arn:?}"
    )
}

/// `PutResourcePolicy` whose policy document is not a JSON object. NOT recorded: what
/// is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the sentence names the member the document arrived in,
/// which the model describes as "the policy document to set, in JSON format".
pub const POLICY_MUST_BE_JSON: &str = "PolicyInJson must be a policy document in JSON format.";

/// `PutResourcePolicy` for a resource this endpoint cannot take another policy for.
/// Its own capacity, not an AWS quota, so the sentence says which ceiling was reached.
pub fn too_many_resource_policies(ceiling: usize) -> String {
    format!(
        "this endpoint holds resource policies for {ceiling} resources, which is as many \
             as it can hold"
    )
}

/// `CreateUsageProfile` for a name this account already holds a profile under.
/// RECORDED template: the sentence names the resource kind and carries the `Name` the
/// request itself sent, which is the whole address of a usage profile. Unlike the
/// catalog duplicates above, this one DOES echo the requested name, because that is
/// the wording the corpus pins for this resource.
pub fn usage_profile_already_exists(name: &str) -> String {
    format!("UsageProfile {name} already exists.")
}

/// A usage profile lookup (`GetUsageProfile`, and every other operation that addresses
/// a profile by name) for a name no profile of this account has. RECORDED template,
/// including its wording, built from the requested `Name`.
///
/// The UPDATE words the same miss differently — it closes the sentence with a period
/// ([`usage_profile_not_found_on_update`]) — which is also recorded, so the two are kept apart
/// rather than folded into one wording.
pub fn usage_profile_not_found(name: &str) -> String {
    format!("UsageProfile {name} does not exists")
}

/// The same miss as [`usage_profile_not_found`], as `UpdateUsageProfile` words it: RECORDED
/// template, the same sentence closed with a period. The variable part is the requested `Name`.
pub fn usage_profile_not_found_on_update(name: &str) -> String {
    format!("UsageProfile {name} does not exists.")
}

/// A session create for an `Id` this Region already holds a session under — whatever the
/// rest of the request carries: the same definition again, a different one, or a different
/// idempotency token.
///
/// RECORDED template: the fixed text `Session already created, sessionId=` and ONE variable
/// part, the `Id` the request sent, taken verbatim (no trailing full stop). The id is the
/// whole address of a session and the client's own value, so the sentence states which
/// create collided.
pub fn session_already_exists(id: &str) -> String {
    format!("Session already created, sessionId={id}")
}

/// A session create whose `GlueVersion` fails the restriction the model states for it
/// ([`super::session_glue_version_defect`]). NOT recorded: what is pinned is the
/// behaviour — 400 with the create's modelled `InvalidInputException` — and a sentence
/// carrying the value the request sent plus the bound it has to clear, as
/// [`malformed_identity_center_instance_arn`] does for its own member.
pub fn session_glue_version_too_low(glue_version: &str) -> String {
    let minimum = super::MIN_SESSION_GLUE_VERSION;
    format!(
        "GlueVersion {glue_version} is not supported for sessions, whose GlueVersion \
             must be greater than {minimum:.1}"
    )
}

/// A session create this endpoint cannot take another session for. Its own capacity,
/// not an AWS quota, so the sentence says which ceiling was reached.
pub fn too_many_sessions(ceiling: usize) -> String {
    format!("this endpoint holds {ceiling} sessions, which is as many as it can hold")
}

/// A session request that cannot act on a session resting in, or being moved into, a
/// status that leaves it nothing to act on, which is what `DeleteSession` refuses with its
/// modelled `IllegalSessionStateException` and what `StopSession` refuses a session with no
/// runtime up with. RECORDED: the sentence is fixed text around the status the session is
/// actually in, read off the store, and names no session — the caller already knows which
/// id it asked about.
pub fn session_in_illegal_state(status: &str) -> String {
    format!("Session is in {status} status")
}

/// A statement run on a session whose `SessionStatus` is not
/// [`super::SESSION_STATUS_READY`], which is the only status a remote Spark runtime
/// can take code in.
///
/// RECORDED template: the fixed sentence around its two variable parts, written as the
/// service words them — the `SessionId` the request addressed and the status the session is
/// actually in, read off the store, each stated as `<member>=<value>`. The behaviour it goes
/// with is 400 with the run's modelled `InvalidInputException`, which is the refusal this
/// service is recorded answering a run on a session that is not ready with, whether the
/// service is still bringing its runtime up and the status may yet change or the session has
/// ENDED ([`super::session_has_ended`]) and its id names no runtime at all.
pub fn session_not_ready(id: &str, status: &str) -> String {
    format!("Session is not ready, session_id={id}, status={status}")
}

/// A statement read (`GetStatement`, and every other operation that addresses a
/// statement by its session and `Id`) for an id OUTSIDE the id space of the session it
/// was asked for ([`super::SessionEntry::statement_id_in_live_space`]): no session mints
/// it, so it names no statement at all.
///
/// RECORDED template: fixed text around its ONE variable part, the statement `Id` the
/// request asked for, taken exactly as sent. The session is NOT named — the caller
/// already addressed it — which is why this takes the id alone.
pub fn statement_not_found(id: i32) -> String {
    format!("Statement {id} not found.")
}

/// A statement read for an id the session has not minted but that is INSIDE its live id
/// space ([`super::SessionEntry::statement_id_in_live_space`]): the id names a statement
/// slot of this session's own runtime, so what the read cannot serve is about the
/// session's state rather than about an id nothing could ever hold.
///
/// RECORDED constant, and the whole message: the sentence has no variable part at all —
/// neither the session nor the id appears in it. It is therefore not the wording a
/// session request resting in an unusable status is refused with
/// ([`session_in_illegal_state`]), which names that status.
pub const STATEMENT_ILLEGAL_SESSION_STATUS: &str = "Illegal Session Status";

/// A usage profile write whose `ConfigurationObject` sets a `DefaultValue` above the
/// `MaxValue` it sets for the same parameter. RECORDED template: the sentence carries
/// the parameter key, the `ProfileConfiguration` member its map arrived under, and the
/// `MaxValue` exactly as submitted.
pub fn configuration_default_above_max(
    parameter: &str,
    section: &str,
    max_value: &str,
) -> String {
    format!(
        "default value for parameter {parameter} in {section} should be less than or \
             equal to max value {max_value}."
    )
}

/// The same refusal for the OTHER bound of the same parameter: a `DefaultValue` below
/// its `MinValue`. NOT recorded on its own \u2014 what is pinned is the coherence rule and
/// the sentence above \u2014 so this states the mirror requirement in that same wording,
/// carrying the parameter, its section and the `MinValue` as submitted.
pub fn configuration_default_below_min(
    parameter: &str,
    section: &str,
    min_value: &str,
) -> String {
    format!(
        "default value for parameter {parameter} in {section} should be greater than or \
             equal to min value {min_value}."
    )
}

/// The same refusal for the two bounds themselves: a `MinValue` above the `MaxValue`
/// of the same parameter, which is a range no value lies in. NOT recorded on its own,
/// so it follows the wording of [`configuration_default_above_max`] with the member
/// that is too large named.
pub fn configuration_min_above_max(parameter: &str, section: &str, max_value: &str) -> String {
    format!(
        "min value for parameter {parameter} in {section} should be less than or equal \
             to max value {max_value}."
    )
}

/// A usage profile whose JOB configuration defaults exactly one of the two parameters that
/// together state a job's capacity — the worker type and the number of workers
/// ([`super::security_configurations`] holds the check).
///
/// RECORDED constant, and the whole message: it states the rule rather than the request, so
/// it names neither the parameter the configuration did default nor the value it gave it, and
/// one wording answers either half of the pair being the one supplied.
pub const JOB_WORKER_DEFAULTS_INCOMPLETE: &str =
    "Please set default value for both Worker type and Number of workers in jobs \
     configuration, if you intend to set default for either parameter.";

/// A `PutDataCatalogExportConfiguration` whose `EncryptionConfiguration` is not the
/// encryption the table bucket the export writes into is already protected by
/// ([`super::export_encryption_differs`]): the bucket's encryption is the bucket's own, so a
/// configuration that disagrees with it cannot be applied and the write is refused rather
/// than stored.
///
/// RECORDED template: the refusal text is itself a JSON document carrying the sentence under
/// a `Message` key, the way this surface's other documents are
/// ([`super::glossary_already_exists_message`]), and the sentence names the bucket the export
/// writes into — which is the service's own bucket for it
/// ([`super::EXPORT_TABLE_BUCKET_NAME`]) and the one variable part, since no request names a
/// bucket.
pub fn export_table_bucket_encryption_conflict(table_bucket_name: &str) -> String {
    super::catalog_refusal_document(&format!(
        "The existing table bucket {table_bucket_name} uses a different encryption \
             configuration than the request."
    ))
}

/// A `PutDataCatalogExportConfiguration` whose `ExportEncryptionConfiguration` names an
/// `SseAlgorithm` that is not one of the two the member documents
/// ([`super::EXPORT_SSE_ALGORITHMS`]). NOT recorded: what is pinned is the behaviour — 400
/// with the operation's modelled `InvalidInputException` — and a sentence carrying the value
/// as submitted and the values the member admits.
pub fn unsupported_export_sse_algorithm(value: &str) -> String {
    format!(
        "SseAlgorithm {value} is not supported for the data catalog export; supported \
             values are {}",
        super::EXPORT_SSE_ALGORITHMS.join(" and ")
    )
}

/// A partition listing (`GetPartitions`) whose `Expression` addresses a table that declares
/// NO partition keys.
///
/// A filter selects among the keys a partition is addressed by, so a table that declares
/// none offers no key collection to read one against, and the read fails on that absent
/// collection rather than on the expression. The whole sentence is FIXED and names nothing
/// of the request: neither the expression, nor the table, nor a column.
pub const PARTITION_FILTER_ON_UNPARTITIONED_TABLE: &str =
    "Cannot invoke \"java.util.Collection.toArray()\" because \"elements\" is null";

/// A partition listing (`GetPartitions`, and the Lake Formation application-integration
/// listing that carries the same member) whose `Expression` is no filter the service can
/// evaluate over the keys the addressed table DECLARES
/// ([`super::ParsedPartitionFilter::parse`], [`super::ParsedPartitionFilter::bind`]).
///
/// RECORDED constant, and the ONE wording every such expression is refused with: a predicate
/// that is no `WHERE`-clause expression at all, one carrying an operator the member does not
/// document, one reading a column the table does not have, one reading a real column that is
/// no partition key of it, and one filtering a key of a type a filter cannot compare all
/// answer this sentence. It is fixed text in full — it carries neither the expression, nor
/// the column, nor the operators a filter is built from — so the refusal discloses nothing
/// about which part of the filter could not be run.
///
/// What it does NOT cover is the comparison whose LITERAL is no value of the addressed key's
/// declared type, which is recorded as its own refusal class
/// ([`partition_value_not_of_key_type`]).
pub const INVALID_PARTITION_EXPRESSION: &str = "Invalid partition expression!";

/// The server-side exception class this service names when a partition-key comparison
/// addresses a literal that is no value of that key's declared type. The refusal of such a
/// comparison is reported as that class name, then the offending literal, then the type the
/// literal could not be read as — a different class from the expression-level refusal
/// ([`INVALID_PARTITION_EXPRESSION`]), which names no class at all.
const INVALID_PARTITION_VALUE_CLASS: &str =
    "com.amazonaws.daylight.core.exceptions.InvalidPartitionValueException";

/// A partition listing whose `Expression` compares a partition key against a literal that
/// is no value of that key's declared type.
///
/// The FIXED text is the class name above, the connective " is not a " and the closing stop;
/// the VARIABLE parts are `literal`, the offending literal as the request's `Expression`
/// spells it, and `declared`, the type the addressed partition key is stored with. The key's
/// own name is not part of the sentence.
pub fn partition_value_not_of_key_type(literal: &str, declared: &str) -> String {
    format!("{INVALID_PARTITION_VALUE_CLASS}: {literal} is not a {declared}.")
}

/// A partition listing whose `Segment` names a segment outside the scan it splits the
/// table into: "the zero-based index number of the segment. For example, if the total
/// number of segments is 4, SegmentNumber values range from 0 through 3"
/// (`Segment$SegmentNumber`).
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the sentence states that documented rule, carrying the
/// segment the request asked for and the number of segments it split the table into.
pub fn segment_number_outside_total(segment_number: i32, total_segments: i32) -> String {
    format!(
        "SegmentNumber {segment_number} is not a segment of a scan of {total_segments} \
             segments; SegmentNumber is zero-based and ranges from 0 through {}",
        total_segments.saturating_sub(1)
    )
}

/// A crawl listing (`ListCrawls`) whose `Filters` states a filter missing one of the three
/// members a comparison is made of: `Filters` is "a list of fields, comparators and value
/// that you can use to filter the crawler runs" (`CrawlsFilter`), so a filter without a
/// field name, a comparator or a value compares nothing.
///
/// NOT recorded: what is pinned is the behaviour — 400 with the operation's modelled
/// `InvalidInputException` — and the sentence states the rule the filter broke, naming the
/// member it left out.
pub fn crawls_filter_without_member(member: &str) -> String {
    format!(
        "Filters states a filter without {member}; a filter on a crawler's runs is a \
             FieldName, a FilterOperator and a FieldValue to compare against"
    )
}

/// A crawl listing whose `Filters` keys on a field that is no field of a crawl. NOT
/// recorded: the behaviour is the operation's modelled `InvalidInputException`, and the
/// sentence carries the field name as submitted beside the fields a crawl is filtered on
/// ([`crate::ops::types::FIELD_NAME_VALUES`]).
pub fn crawls_filter_field_unknown(field: &str) -> String {
    format!(
        "Filters states FieldName {field}, which is not a field of a crawl; the fields a \
             crawler's runs are filtered on are {}",
        crate::ops::types::FIELD_NAME_VALUES.join(", ")
    )
}

/// A crawl listing whose `Filters` compares with a comparator the model does not declare.
/// NOT recorded: the behaviour is the operation's modelled `InvalidInputException`, and the
/// sentence carries the comparator as submitted beside the ones a filter compares with
/// ([`crate::ops::types::FILTER_OPERATOR_VALUES`]).
pub fn crawls_filter_operator_unknown(operator: &str) -> String {
    format!(
        "Filters states FilterOperator {operator}, which is not a comparator; a filter on \
             a crawler's runs compares with {}",
        crate::ops::types::FILTER_OPERATOR_VALUES.join(", ")
    )
}

/// A crawl listing whose `Filters` compares a NUMERIC field of a crawl against a value that
/// is no number — `START_TIME` and `END_TIME` are "the epoch timestamp in milliseconds" and
/// `DPU_HOUR` "the number of data processing unit (DPU) hours used for the crawl"
/// (`CrawlsFilter$FieldName`).
///
/// NOT recorded: the behaviour is the operation's modelled `InvalidInputException`, and the
/// sentence carries the field and the value as submitted.
pub fn crawls_filter_value_not_a_number(field: &str, value: &str) -> String {
    format!("Filters compares {field} against {value}, which is not a number")
}
