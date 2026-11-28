Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 36f53ddf6ade8f29c2c5a7f31a2490a14197fa50

# A database named by RegisterApplication$DatabaseArn resolves at any regional endpoint, breaking holds_database/holds_resource's documented Region scoping

_Filed: 2026-11-28T10:48:06Z_
_Operation: PutResourcePermission | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `36f53ddf6ade8f29c2c5a7f31a2490a14197fa50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegionState::holds_database` and `RegionState::holds_resource` are the two places this crate decides whether an ARN names something it holds, and both document that the `region` argument is what keeps a resource of ANOTHER Region from resolving. Neither of them, nor the resolver they delegate to (`RegionState::resolve_database`), ever compares the Region a candidate ARN embeds against the partition's Region.

The Region argument is used only to MINT an ARN for a database derived from a credential's `DatabaseName` (`database_arn(region, ...)`). For the OTHER way a registration names a database — `RegisterApplication$DatabaseArn` / `UpdateApplicationSettings$DatabaseArn` — `StoredApplication::databases` keeps the supplied ARN verbatim (`arn: arn.to_string()`), and `resolve_database` matches it with a bare string comparison (`database_arn.is_none_or(|arn| database.arn == arn)`). `arn_names_a_database` accepts any `ssm-sap` ARN whose resource type is `database`, whatever Region it carries.

So a registration made at the us-east-1 endpoint that names `DatabaseArn: arn:aws:ssm-sap:eu-west-1:<account>:database/other-app/HDB` makes that eu-west-1 ARN resolve at the us-east-1 endpoint:

* `PutResourcePermission`, `GetResourcePermission` and `DeleteResourcePermission` accept it as their `ResourceArn` and write/read a RESTORE policy for it in the us-east-1 partition — although `holds_database`'s own doc says "a database of another Region resolves to nothing here".
* `TagResource`, `UntagResource` and `ListTagsForResource` accept it too (it is in `StoredApplication::resource_arns`, which is what `holds_resource` enumerates) — although `holds_resource`'s own doc says "an ARN of another Region therefore resolves to nothing here, exactly as an application of another Region does".

The grants and tags are then keyed, inside the us-east-1 partition, by an ARN that spells eu-west-1, and the very same ARN presented at the eu-west-1 endpoint resolves to nothing — so the two endpoints disagree about the one resource, which is exactly the condition both doc comments say cannot arise.

## Evidence

The rule the code is supposed to hold, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2369-2383`:
```rust
    /// Whether `arn` names a SAP HANA DATABASE this Region holds.
    ///
    /// Narrower than [`Self::holds_resource`] on purpose: the resource-permission calls
    /// are database-scoped, so the ARN they take has to name one of the databases the
    /// registrations of this Region named ...
    ///
    /// `region` is the Region this partition holds, which is the Region those ARNs embed,
    /// so a database of another Region resolves to nothing here.
    pub fn holds_database(&self, region: &str, arn: &str) -> bool {
        self.resolve_database(region, None, None, None, Some(arn))
            .is_some()
    }
```
and the same claim for the tagging routes' resolver, `src/state.rs:2350-2353`:
```rust
    /// `region` is the Region this partition holds, which is the Region those ARNs
    /// embed: an ARN of another Region therefore resolves to nothing here, exactly as
    /// an application of another Region does ([`Self::resolve_application_id`]).
    pub fn holds_resource(&self, region: &str, arn: &str) -> bool {
```

The code that breaks both — `region` is threaded through but never compared against the candidate ARN, `src/state.rs:2424-2435`:
```rust
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
```

What puts a foreign-Region ARN into that set — `StoredApplication::databases`, `src/state.rs:1234-1251`:
```rust
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
```
(`region` is used for the credential-derived branch only: `arn: database_arn(region, &self.id, &database_id)` at `src/state.rs:1260`.)

The kind test that lets any Region through, `src/state.rs:3121-3124`:
```rust
pub fn arn_names_a_database(arn: &str) -> bool {
    ssm_sap_resource_type(arn)
        .is_none_or(|resource_type| resource_type == ARN_RESOURCE_TYPE_DATABASE)
}
```
and the Region reader that exists in this very file but is never consulted here, `src/state.rs:3138`: `pub fn arn_region(arn: &str) -> &str`.

The resource-permission handlers that call it and so inherit the gap: `src/ops/put_resource_permission.rs:191-194`, `src/ops/get_resource_permission.rs:172`, `src/ops/delete_resource_permission.rs:196-199`; the tagging handlers: `src/ops/tag_resource.rs:194-197`, `src/ops/untag_resource.rs:198-201`, `src/ops/list_tags_for_resource.rs:184`.

Test coverage: nothing pins the behaviour. Every `DatabaseArn` the suites REGISTER carries the test Region or no Region at all — `tests/all/resource_permissions.rs:157` (`COLON_SEGMENT_DATABASE_ARN = "arn:aws:rds:us-east-1:000000000000:db:sap-prd"` with `REGION = "us-east-1"` at :59), `tests/all/get_database.rs:260`, `tests/all/stop_connected_entity.rs:140`, `tests/all/component_cross_references.rs:202,218`, `tests/all/configuration_check_rules.rs:305`. The only `eu-west-1` ssm-sap ARNs in the suites are `SourceResourceArn` values (`tests/all/resource_permissions.rs:121`, `tests/all/delete_resource_permission.rs:133`), which both handlers deliberately do NOT resolve against the store. The "an ARN carrying another Region" rows in `tests/all/tag_resource.rs:432`, `tests/all/untag_resource.rs:436`, `tests/all/resource_tags.rs:376` and `tests/all/delete_resource_permission.rs:537` use ARNs that no registration named in `DatabaseArn`, so they resolve to nothing for a reason unrelated to the Region check.

## Steps to Reproduce

1. POST /register-application signed for us-east-1 with {"ApplicationId":"LOCAL","ApplicationType":"SAP_ABAP","Instances":["i-0123456789abcdef0"],"DatabaseArn":"arn:aws:ssm-sap:eu-west-1:000000000000:database/far-app/HDB"} -> 200
2. POST /list-databases signed for us-east-1 with {"ApplicationId":"LOCAL"} -> 200 reporting one database whose Arn is the eu-west-1 ARN
3. POST /put-resource-permission signed for us-east-1 with {"ActionType":"RESTORE","ResourceArn":"arn:aws:ssm-sap:eu-west-1:000000000000:database/far-app/HDB","SourceResourceArn":"arn:aws:ssm-sap:us-east-1:000000000000:database/src/HDB"} -> 200 with a rendered Policy. Expected per src/state.rs:2378-2380 ("a database of another Region resolves to nothing here"): the modelled ValidationException from crate::state::invalid_database_arn
4. POST /tags/arn%3Aaws%3Assm-sap%3Aeu-west-1%3A000000000000%3Adatabase%2Ffar-app%2FHDB signed for us-east-1 with {"tags":{"env":"prod"}} -> 200, and GET the same path -> 200 with those tags. Expected per src/state.rs:2350-2352 ("an ARN of another Region therefore resolves to nothing here"): HTTP 404 with crate::state::RESOURCE_NOT_FOUND_MESSAGE
5. POST /get-resource-permission signed for eu-west-1 with the same ResourceArn -> 400: the two regional endpoints disagree about the one resource, and the grant and tag sit in the us-east-1 partition under an ARN that spells eu-west-1
