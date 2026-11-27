// SPDX-License-Identifier: CC0-1.0

//! Access to a Glue USER-DEFINED FUNCTION: the ARN that names one, the permission a caller
//! has to hold on it, and the session contexts and temporary credentials Lake Formation
//! vends for it.
//!
//! A user-defined function is a Data Catalog resource like a database or a table, so the
//! two operations that hand out access to one — `GetGlueFunctionSessionContext` and
//! `GetTemporaryGlueFunctionCredentials` — resolve it the same way a grant on it is
//! resolved: the ARN names a catalog, a database and a function, the database is read from
//! GLUE's own store ([`super::missing_grant_entity`]), and what the caller may do with the
//! function is read from the permission records this service holds. Both operations ask
//! those questions HERE, so they cannot answer them differently.
//!
//! "You must have the EXECUTE permission on every function in the list"
//! (`GetGlueFunctionSessionContext`) and "you must have the EXECUTE permission on the
//! function" (`GetTemporaryGlueFunctionCredentials`) are the rule both apply, and
//! [`EXECUTE_PERMISSION`] is the value of the `Permission` enum that names it.

use std::time::Duration;

use super::*;

/// The AWS service a user-defined function belongs to: Glue, which owns the Data Catalog
/// the function is defined in.
const GLUE_SERVICE: &str = "glue";

/// The resource type a user-defined function's ARN names, as the model spells it: the ARN
/// is "in the format arn:aws:glue:region:account-id:userDefinedFunction/database-name/
/// function-name"
/// (`smithy_query(query="doc",shape="GetTemporaryGlueFunctionCredentialsRequest",
/// member="UserDefinedFunctionArn")`).
const FUNCTION_ARN_RESOURCE_TYPE: &str = "userDefinedFunction";

/// The permission that lets its holder run a user-defined function and fetch its
/// artifacts: "EXECUTE – Use the function in a job, and get credentials to download its
/// artifacts." (`smithy_query(query="doc",shape="Resource",member="UserDefinedFunction")`),
/// a value of the `Permission` enum (`crate::ops::types::PERMISSION_VALUES`).
pub const EXECUTE_PERMISSION: &str = "EXECUTE";

/// The permission that IS every permission on the resource it is held on: "ALL – All of the
/// preceding permissions."
/// (`smithy_query(query="doc",shape="Resource",member="UserDefinedFunction")`), so a holder
/// of it holds [`EXECUTE_PERMISSION`] too.
pub const SUPER_PERMISSION: &str = "ALL";

/// The user-defined function one ARN names: the Data Catalog it is defined in, the
/// database that contains it, and its own name.
///
/// The database name is folded the way the Data Catalog stores it
/// (`glue_state::fold_name`, "for Hive compatibility, this is folded to lowercase when it
/// is stored"), so an ARN and a grant that spell it differently name one database. The
/// FUNCTION name is kept as written: the catalog documents no folding for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDefinedFunctionRef {
    /// The Data Catalog the function belongs to: the account id field of the ARN, else the
    /// account's own catalog when the ARN leaves it empty.
    pub catalog_id: String,
    /// The database that contains the function, folded.
    pub database_name: String,
    /// The function's own name, as the ARN wrote it.
    pub name: String,
}

impl UserDefinedFunctionRef {
    /// This function as the modelled `Resource` a permission on it is held on, which is
    /// what [`super::missing_grant_entity`] resolves and what a grant record carries.
    pub fn resource(&self) -> Resource {
        Resource {
            user_defined_function: Some(UserDefinedFunctionResource {
                catalog_id: Some(self.catalog_id.clone()),
                database_name: self.database_name.clone(),
                name: Some(self.name.clone()),
                user_defined_function_wildcard: None,
            }),
            ..Resource::default()
        }
    }
}

/// The function `arn` names, or the refusal a value that names none draws
/// ([`UNSUPPORTED_RESOURCE_ARN`]).
///
/// `ResourceArnString` carries no pattern in the model, so the boundary validator admits
/// any string and this is what refuses a value that is not the ARN of a user-defined
/// function: the six-field ARN layout ([`validate_resource_arn`]), the `glue` service that
/// owns the function, and a resource field of exactly
/// `userDefinedFunction/<database>/<function>` with both names non-empty. Nothing is
/// indexed or unwrapped — a malformed value is answered, never panicked on.
pub fn read_user_defined_function_arn(arn: &str) -> Result<UserDefinedFunctionRef, String> {
    validate_resource_arn(arn)?;
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let (Some(service), Some(account), Some(resource)) =
        (fields.get(2), fields.get(4), fields.get(5))
    else {
        return Err(UNSUPPORTED_RESOURCE_ARN.to_string());
    };
    if *service != GLUE_SERVICE {
        return Err(UNSUPPORTED_RESOURCE_ARN.to_string());
    }
    let mut parts = resource.split('/');
    let (Some(resource_type), Some(database), Some(name), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(UNSUPPORTED_RESOURCE_ARN.to_string());
    };
    if resource_type != FUNCTION_ARN_RESOURCE_TYPE || database.is_empty() || name.is_empty() {
        return Err(UNSUPPORTED_RESOURCE_ARN.to_string());
    }
    Ok(UserDefinedFunctionRef {
        // "The ID of the Data Catalog. By default, it's your account ID."
        // (`UserDefinedFunctionResource$CatalogId`): the account the ARN names, else the
        // account's own catalog for a partition-local ARN that leaves the field empty.
        catalog_id: resource_catalog(Some(account), &resolve_catalog_id(None)),
        database_name: glue_state::fold_name(database),
        name: name.to_string(),
    })
}

/// Whether `caller` holds `permission` on `function`.
///
/// Two ways to hold it, and nothing else:
///
/// * the caller is an administrator of the function's Data Catalog
///   ([`is_data_lake_administrator`]), which holds every permission on every resource of
///   that catalog implicitly;
/// * a permission record of this store covers the function and carries `permission` — in
///   the caller's own name or through a group it belongs to
///   ([`effective_principal_holds`]).
///
/// Reading only: it decides from the records the store holds and writes nothing.
pub fn holds_function_permission(
    store: &ComAmazonawsLakeformationAwslakeformationState,
    caller: &str,
    function: &UserDefinedFunctionRef,
    permission: &str,
) -> bool {
    if is_data_lake_administrator(store, &function.catalog_id, caller) {
        return true;
    }
    store.permission_grants.values().any(|grant| {
        effective_principal_holds(caller, &grant.principal)
            && grant_covers_function(&grant.resource, function)
            && grant_carries(grant, permission)
    })
}

/// Whether `grant` hands out `permission`: it carries the value itself, or the
/// [`SUPER_PERMISSION`] that is every permission on the resource.
///
/// Both lists are weighed: a permission held WITH the grant option is held.
fn grant_carries(grant: &PermissionGrant, permission: &str) -> bool {
    grant
        .permissions
        .iter()
        .chain(grant.permissions_with_grant_option.iter())
        .any(|held| held == permission || held == SUPER_PERMISSION)
}

/// Whether a permission record held on `resource` covers `function`.
///
/// A record on another kind of resource covers no function: a permission on the DATABASE
/// that contains it is held over the database, not over the functions in it. A record on
/// the database's function WILDCARD does cover it — "a wildcard object representing every
/// user-defined function in the database, including functions created after the grant"
/// (`smithy_query(query="doc",shape="UserDefinedFunctionResource",
/// member="UserDefinedFunctionWildcard")`).
fn grant_covers_function(resource: &Resource, function: &UserDefinedFunctionRef) -> bool {
    let Some(granted) = resource.user_defined_function.as_ref() else {
        return false;
    };
    if resource_catalog(granted.catalog_id.as_deref(), &function.catalog_id)
        != function.catalog_id
    {
        return false;
    }
    if glue_state::fold_name(&granted.database_name) != function.database_name {
        return false;
    }
    match granted.name.as_deref().filter(|name| !name.is_empty()) {
        Some(name) => name == function.name,
        None => granted.user_defined_function_wildcard.is_some(),
    }
}

/// Whether `principal` administers the data lake of `catalog_id`.
///
/// "A list of Lake Formation principals" designated as data lake administrators
/// (`DataLakeSettings$DataLakeAdmins`) and the read-only administrators beside them
/// (`DataLakeSettings$ReadOnlyAdmins`, "with only view access to the resources") hold every
/// permission of the catalog implicitly, so a decision about one of its resources asks the
/// settings before it walks the records.
pub fn is_data_lake_administrator(
    store: &ComAmazonawsLakeformationAwslakeformationState,
    catalog_id: &str,
    principal: &str,
) -> bool {
    let settings = store.data_lake_settings_for(catalog_id);
    let designated = [&settings.data_lake_admins, &settings.read_only_admins]
        .into_iter()
        .flatten()
        .flatten()
        .any(|admin| admin.data_lake_principal_identifier.as_deref() == Some(principal));
    designated
}

/// One session context Lake Formation handed out: who asked for it, the functions it
/// covers, and when the clock makes it spent.
///
/// `GetGlueFunctionSessionContext` is what mints one — "returns a session context for one
/// or more user-defined functions that an integrated analytics engine runs together in a
/// job" — so the record is the session context itself rather than a report of one.
#[derive(Debug, Clone)]
pub struct GlueFunctionSessionContext {
    /// The principal the context was handed to.
    pub principal: String,
    /// The functions it covers, as the request named them.
    pub user_defined_function_arns: Vec<String>,
    /// "The date and time when the session context expires."
    pub expiration: SystemTime,
}

/// How long a session context nothing asked a duration for is valid: "The number of
/// seconds the session context is valid. The default is 86,400 (24 hours)."
/// (`smithy_query(query="doc",shape="GetGlueFunctionSessionContextRequest",
/// member="DurationSeconds")`).
pub const DEFAULT_SESSION_CONTEXT_SECONDS: i32 = 86_400;

/// The most session contexts this store holds at once.
///
/// A context is request-driven — one per call, keyed by a minted id — so without a ceiling
/// a caller that keeps asking would grow the store without end. Contexts the clock has
/// expired are dropped first ([`expire_session_contexts`]), so this bounds how many LIVE
/// ones one emulator holds; passing it answers the modelled `InvalidInputException`
/// `GetGlueFunctionSessionContext` declares (it declares no quota error of its own).
/// Emulator-chosen, like this store's other ceilings.
pub const MAX_GLUE_FUNCTION_SESSION_CONTEXTS: usize = 1_000;

/// What a call that would open one session context too many is refused with.
///
/// Emulator-worded, like the other ceiling refusals of this store: the model fixes that
/// there is a limit, not the sentence.
pub fn too_many_session_contexts(held: usize) -> String {
    format!(
        "{held} session contexts are already held, the maximum is \
         {MAX_GLUE_FUNCTION_SESSION_CONTEXTS}"
    )
}

/// A fresh session context id: a version-4 UUID, rendered canonically.
///
/// `SessionContextIdString` carries no pattern, so the form is this service's own choice,
/// and it is one no caller can predict or collide with. The randomness is the `uuid`
/// crate's, reached through the telemetry crate's uuid-backed id newtype this crate already
/// depends on — nothing here is a hand-rolled generator. This names a session context, not
/// a credential: a credential comes from the emulator's credential store.
pub fn new_session_context_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// One set of temporary credentials Lake Formation vended for a function's artifacts: the
/// access key id it is named by, what it was vended for, and when the clock makes it spent.
///
/// The secrets are NOT kept here: they are the emulator credential store's, and the
/// response that minted them is the only place they are reported.
#[derive(Debug, Clone)]
pub struct VendedFunctionCredentials {
    /// The access key id the emulator's credential store minted, which this record is
    /// keyed by.
    pub access_key_id: String,
    /// The function whose artifacts the credentials reach.
    pub user_defined_function_arn: String,
    /// The principal they were vended to.
    pub principal: String,
    /// "The date and time when the temporary credentials expire."
    pub expires_at: SystemTime,
}

/// How long the credentials for a function's artifacts are valid.
///
/// The operation models no duration member and the model documents none, so this span is
/// the emulator's own choice; what the model fixes is that the credentials EXPIRE, which
/// `Expiration` reports and [`expire_vended_function_credentials`] honours.
pub const FUNCTION_CREDENTIALS_DURATION: Duration = Duration::from_secs(3_600);

/// The most live vended credential sets this store holds at once, for the same reason
/// [`MAX_GLUE_FUNCTION_SESSION_CONTEXTS`] bounds the contexts: one call mints one set.
pub const MAX_VENDED_FUNCTION_CREDENTIALS: usize = 1_000;

/// What a call that would mint one credential set too many is refused with.
pub fn too_many_vended_credentials(held: usize) -> String {
    format!(
        "{held} sets of temporary credentials are already held, the maximum is \
         {MAX_VENDED_FUNCTION_CREDENTIALS}"
    )
}

/// `at` advanced by `duration` — the one way this service stamps an expiry.
///
/// `SystemTime + Duration` PANICS on a sum the platform's clock type cannot represent, so
/// an expiry is computed here instead: an unrepresentable sum saturates at `at`, which no
/// request can read as still valid.
pub fn expiry_after(at: SystemTime, duration: Duration) -> SystemTime {
    at.checked_add(duration).unwrap_or(at)
}

/// The duration a session-context request asks for: the seconds it named, else
/// [`DEFAULT_SESSION_CONTEXT_SECONDS`].
///
/// The modelled range (`SessionDurationSecondInteger`, 60..86400) is refused at the request
/// boundary before a handler runs, so a value that arrives here is inside it; a negative
/// value could not be, and saturates at zero rather than wrapping.
pub fn session_context_duration(requested: Option<i32>) -> Duration {
    let seconds = requested.unwrap_or(DEFAULT_SESSION_CONTEXT_SECONDS);
    Duration::from_secs(u64::try_from(seconds).unwrap_or_default())
}

impl ComAmazonawsLakeformationAwslakeformationState {
    /// Drops every session context the clock has made spent at `now`.
    ///
    /// The expiry is due at its own instant — "the date and time when the session context
    /// expires" — so this materialises only what the clock already owed: no response
    /// changes because it ran, and a context still inside its duration is untouched.
    pub fn expire_session_contexts(&mut self, now: SystemTime) {
        self.glue_function_session_contexts
            .retain(|_, context| context.expiration > now);
    }

    /// The access key ids of the vended credential sets the clock has made spent at `now`,
    /// dropping their records.
    ///
    /// Owed at the expiry the response that minted them advertised, for the same reason as
    /// [`Self::expire_session_contexts`]; the caller forgets the credential itself in the
    /// emulator's credential store, so the emulator stops accepting it for signing then.
    pub fn expire_vended_credentials(&mut self, now: SystemTime) -> Vec<String> {
        let mut spent = Vec::new();
        self.vended_function_credentials.retain(|_, vended| {
            let live = vended.expires_at > now;
            if !live {
                spent.push(vended.access_key_id.clone());
            }
            live
        });
        spent
    }
}

/// Forget every vended credential set the clock has made spent at `now`: drop its record
/// here and delete the credential it names from the emulator's credential store.
///
/// A store that no longer holds the credential is already in the state this asks for, so
/// the delete's error is nothing to report.
pub fn expire_vended_function_credentials(
    store: &mut ComAmazonawsLakeformationAwslakeformationState,
    credential_store: &::emulator_server::CredentialStore,
    now: SystemTime,
) {
    for access_key_id in store.expire_vended_credentials(now) {
        let _ = credential_store.delete(&access_key_id);
    }
}
