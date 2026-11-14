// SPDX-License-Identifier: CC0-1.0

//! What a statement can reach BEYOND its own database: the services whose
//! resources Amazon Redshift's data-loading, data-export and Spectrum
//! statements name, and the facts about the warehouse the statement runs on.
//!
//! `COPY` reads objects out of Amazon S3 and `UNLOAD` writes them there, under
//! an IAM role whose permissions decide whether the read or the write may
//! happen at all; a Spectrum external schema is a database of the AWS Glue Data
//! Catalog, and the tables it holds are read from the objects the catalog says
//! they live in. None of those is this service's own state, so each one is
//! reached on the SAME engine this service is mounted on — the store the owning
//! service's own operations read and write, so an object a caller staged with
//! `PutObject` is the object a `COPY` loads, and an object an `UNLOAD` wrote is
//! the one `GetObject` serves.
//!
//! The handles are resolved ONCE, by the operation that accepted the
//! submission, and carried into the execution: the engine runs a statement on a
//! blocking pool, where there is no request to ask anything over, so what the
//! statement may reach has to travel with it. A service that is not mounted
//! beside this one is `None`, and every reader of it then establishes nothing
//! rather than inventing an answer — the same rule [`super::warehouse`] follows
//! for a control plane that cannot be asked.

use std::collections::BTreeMap;

/// Amazon S3's own object store on this engine: what `COPY` reads and `UNLOAD`
/// writes.
pub type ObjectStore =
    ::engine::ServiceState<::service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State>;

/// The account's IAM records on this engine: the roles a statement names, and
/// the policies that decide what a load or an export may do under one.
pub type IdentityStore = ::engine::ServiceState<
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State,
>;

/// The AWS Glue Data Catalog on this engine: the external databases a Spectrum
/// schema is bound to, and the tables they hold.
pub type DataCatalog =
    ::engine::ServiceState<::service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState>;

/// Everything one submission's statements may reach outside their own database.
///
/// Not `Debug`: a store HANDLE is a lock over another service's whole state, and
/// rendering it would mean taking that lock from a formatter.
#[derive(Clone, Default)]
pub struct DataPlane {
    /// Amazon S3, or `None` when no object store is mounted beside this
    /// service.
    pub objects: Option<ObjectStore>,
    /// The account's IAM records, or `None` when IAM is not mounted beside this
    /// service.
    pub identities: Option<IdentityStore>,
    /// The Glue Data Catalog, or `None` when Glue is not mounted beside this
    /// service.
    pub catalog: Option<DataCatalog>,
    /// The facts about the warehouse and the submission the statements run
    /// under.
    pub facts: StatementFacts,
}

/// What the statements of one submission know about where they are running: the
/// facts a system table reports about a query, and the parameter values the
/// warehouse's engine is running with.
#[derive(Clone, Debug, Default)]
pub struct StatementFacts {
    /// The Region the submission was addressed in, which is the Region an
    /// object address with no Region of its own is resolved in.
    pub region: String,
    /// The warehouse the statement runs on, as the submission named it: a
    /// cluster identifier or a workgroup name.
    pub warehouse: String,
    /// The database the statement runs in.
    pub database: String,
    /// The database user the submission runs as, as the query log reports it.
    pub user: String,
    /// The backend process id this submission's statements run under, which is
    /// what the query log reports for them.
    pub pid: i64,
    /// The parameter values the warehouse's ENGINE is running with — the values
    /// the control plane that owns the warehouse reports as applied to it, read
    /// by the operation that accepted this submission. A parameter the engine
    /// has not been given a value for is absent, and the published default
    /// stands for it ([`super::session_settings`]).
    pub settings: BTreeMap<String, String>,
}

impl DataPlane {
    /// The data plane one submission's statements run against: the stores this
    /// engine holds, and the facts the submission carries.
    pub fn resolve<Eng: Send + Sync + 'static>(engine: &Eng, facts: StatementFacts) -> Self {
        Self {
            objects: ::service_com_amazonaws_s3_amazons3::state::service_state(engine),
            identities:
                ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(
                    engine,
                ),
            catalog: ::service_com_amazonaws_glue_awsglue::state::service_state(engine),
            facts,
        }
    }
}
