// SPDX-License-Identifier: CC0-1.0

//! AWS Amplify's published default-quota catalog.

use super::*;

/// The `Description` AWS Amplify publishes for a plane of resources counted on its
/// parent — the resources one app may have, and the subdomains one domain may have.
///
/// SOURCE: the recorded default-quota listing of `amplify`, which carries this sentence
/// for the BRANCH plane of an app with that plane's own published name read as the
/// phrase it counts ("The maximum number of branches per app that you can create in this
/// account in the current Region."). The sentence is fixed text around the row's own
/// `QuotaName`, so every row of the class is described through this one function and two
/// rows of it cannot drift apart. A row whose published name does NOT read as such a
/// phrase — the hourly app-creation rate and the cache-artifact size below — carries the
/// sentence that listing states for its own code instead (see
/// [`super::maximum_number_phrase`]).
fn amplify_app_plane_description(quota_name: &str) -> String {
    format!(
        "{} that you can create in this account in the current Region.",
        maximum_number_phrase(&quota_name.to_lowercase())
    )
}

/// One AWS Amplify plane counted on its parent: the resources of one kind an app — or a
/// domain — may have.
///
/// Amplify publishes a row of this class per resource a parent holds, with the plane's
/// own name and limit; the class's published members — the countable `None` unit and the
/// sentence above — are the same across it. `Adjustable` is NOT a fact of the class: the
/// evidence behind this catalog publishes the branch plane as fixed and the webhook
/// plane as adjustable, so each row states its own.
fn amplify_app_plane(code: &str, quota_name: &str, value: f64) -> QuotaRow {
    quota(quota_name, value)
        .code(code)
        .describe(&amplify_app_plane_description(quota_name))
}

/// AWS Amplify's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `amplify` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code amplify` reports, which
/// is the "Quotas" table of the AWS Amplify Hosting User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which plane, its
/// published name, unit, default, adjustability and — for the branch plane — its own
/// published sentence.
///
/// Every row is published under the `L-` code AWS mints for it, in code order. The
/// descriptive stand-ins this catalog once published two of these planes under
/// (`AppsPerRegion`, `BranchesPerApp`) are reported by that listing as surplus entries:
/// the branch plane is published under AWS's own code below, and the app count stays
/// unpublished because that listing states no AWS code for it. The further codes that
/// listing carries name no plane this catalog can cite, or state no default for the plane
/// they publish, so they are left to the default-quota-set gap rather than bound to a
/// plane guessed at here.
///
/// The planes that listing does state are published under its codes: the SUBDOMAIN plane
/// of a domain (`L-85685B2E`), the request tokens an app serves per second (`L-CE88B60E`,
/// a quota counted on the APP — so every record of it reports the `RESOURCE` scope and
/// the app's resource type) and the size of a manually deployed ZIP (`L-D95E5509`,
/// published in gigabytes). Where that evidence states a row's name and limit while
/// stating no sentence of its own for it, the row carries no `Description` rather than
/// wording of this catalog's own.
pub(in crate::state) fn amplify_quotas() -> Vec<QuotaRow> {
    vec![
        // How fast apps may be created, which AWS publishes as not adjustable. Its name
        // states the hour it is counted over rather than a plane the class sentence
        // reads off its name, so it carries the sentence the recorded listing states for
        // its own code, which names the console the apps are created in.
        quota("Maximum app creations per hour", 25.0)
            .code("L-2FC3A2FA")
            .fixed()
            .describe(
                "The maximum number of apps that you can create in AWS Amplify Console per \
                 hour in this account in the current Region.",
            ),
        amplify_app_plane("L-4113FC04", "Webhooks per app", 50.0),
        // The SUBDOMAIN plane of a domain: the same counted-plane class as the per-app
        // planes, so it carries that class's sentence around its own name, and AWS
        // publishes it as NOT adjustable.
        amplify_app_plane("L-85685B2E", "Subdomains per domain", 50.0).fixed(),
        // The build cache one environment may hold, published in GIGABYTES and as not
        // adjustable: a size rather than a count, so it carries the sentence the recorded
        // listing states for this code rather than the per-plane class's.
        quota("Environment cache artifact size", 5.0)
            .code("L-A0FC4951")
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size (in GB) of the environment cache artifact."),
        // The branch plane of an app, RE-KEYED onto AWS's own code: the recorded default
        // listing of this service reports the descriptive stand-in this catalog used
        // (`BranchesPerApp`) as a surplus entry and states the code AWS publishes the
        // quota under, that AWS publishes it as NOT adjustable, and its own sentence.
        amplify_app_plane("L-A6716586", "Branches per app", 50.0).fixed(),
        amplify_app_plane("L-AD277529", "Domains per app", 5.0),
        // The request tokens ONE APP serves per second: a quota counted on the app, so
        // every record of it reports the `RESOURCE` scope and the app's resource type,
        // and an increase may be asked for either account-wide or for one app. Its
        // published sentence is not among the data available here, so the row carries
        // none.
        quota("Request tokens per second", 20_000.0)
            .code("L-CE88B60E")
            .adjustable_at(LEVEL_ALL, "AWS::Amplify::App")
            .counted_per_resource(),
        // The manual-deploy archive, published in GIGABYTES like the cache artifact and,
        // like it, as NOT adjustable: a size rather than a count. Its own published
        // sentence is not among the data available here — the recorded listing shows the
        // cache artifact's wording around this row's own name is not what it carries, and
        // withholds the text — so the row publishes none.
        quota("Manual deploy ZIP file size", 5.0)
            .code("L-D95E5509")
            .unit("Gigabytes")
            .fixed(),
    ]
}
