Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicecatalog.execute-deleted-plan-rejected servicecatalog.execute-plan-already-executed-rejected servicecatalog.execute-service-action-not-associated-with-artifact-rejected servicecatalog.execute-service-action-unknown-provisioned-product-rejected
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog.execute-deleted-plan-rejected|_id#2|AssertionError|assert ok, (f"expected an id matching {prefix}-[a-z2-7]{{13}}, got a {type(value).__name__} "
Issue-Key: servicecatalog.execute-plan-already-executed-rejected|_id#2|AssertionError|assert ok, (f"expected an id matching {prefix}-[a-z2-7]{{13}}, got a {type(value).__name__} "
Issue-Key: servicecatalog.execute-service-action-not-associated-with-artifact-rejected|_id#8|AssertionError|assert ok, (f"expected an id matching {prefix}-[a-z2-7]{{13}}, got a {type(value).__name__} "
Issue-Key: servicecatalog.execute-service-action-unknown-provisioned-product-rejected|_id#8|AssertionError|assert ok, (f"expected an id matching {prefix}-[a-z2-7]{{13}}, got a {type(value).__name__} "
Issue-Commit: fe76b837b9c6a96309bb6d3401999300c9d3921a

# Service Catalog minted ids have the wrong shape: 12 base-36 characters instead of AWS's 13 base-32 characters

> _Note: this issue was identified against the code base at commit `fe76b837b9c6a96309bb6d3401999300c9d3921a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProduct

## Description
One net-new root cause in the emulated AWS Service Catalog service (sdkId servicecatalog, crate service-com-amazonaws-servicecatalog-aws242servicecatalogservice), reached by four behavioral checks.

Cause 1 of 1: every Service Catalog identifier the emulator mints is one character too short and drawn from the wrong alphabet, so no minted id has the shape AWS's Service Catalog ids have.

AWS: recorded behavior only — the Smithy model constrains an identifier only loosely (shape com.amazonaws.servicecatalog#Id: string, @length 1..100, @pattern ^[a-zA-Z0-9_\-]*$, targeted by ProductViewSummary.ProductId and ProductViewSummary.Id), so the model does not state the shape. The recorded AWS behavior the suite asserts is that a Service Catalog id is a type prefix, a dash, and exactly THIRTEEN lowercase base-32 characters, i.e. the character class [a-z2-7] (lowercase letters plus the digits 2-7; the digits 0, 1, 8 and 9 never appear). For a product that is `prod-[a-z2-7]{13}` — 18 characters total — and the same rule holds for the other prefixes these checks walk: `prodview-`, `pa-`, `port-`, `cons-`, `plan-`, `pp-`, `rec-`. The variable part is the 13 random base-32 characters; the fixed part is the type prefix and the dash and the LENGTH and ALPHABET of the random part. (No single recorded id value is a requirement — only the template.)

Emulator: CreateProduct returns a ProductId of `prod-` plus TWELVE characters of lowercase base-36 — 17 characters total — and the digits 0, 1, 8 and 9 that base-36 admits are outside AWS's base-32 alphabet. The checks fail with `AssertionError: expected an id matching prod-[a-z2-7]{13}, got a str of length 17` on the ProductId read from CreateProduct's ProductViewDetail.ProductViewSummary.ProductId. Because one shared minting routine is used for every id kind, the same defect applies to every other minted id (product-view, provisioning-artifact, portfolio, constraint, plan, provisioned-product and record ids); the product id is simply the first one each check inspects, so it is where they stop.

Why: a client that validates, parses, pattern-matches, fixed-width-stores or length-checks a Service Catalog id against AWS's shape rejects every id the emulator mints — the id is one character short and can contain characters AWS never emits in one. Any test, schema, database column or regex written against real Service Catalog ids fails against the emulator even though the create itself succeeded, and four behavioral checks cannot get past their very first setup step to exercise the plan/service-action behavior they exist to verify.

Fix: the shared id-minting routine in this service's state (the `mint_id` helper and the `base36_12` encoder it calls, reached from `next_product_id` / `next_product_view_id` / `next_provisioning_artifact_id` / `next_portfolio_id` / `next_constraint_id` and the other `next_*_id` methods) must emit 13 characters from the base-32 alphabet [a-z2-7] after the prefix, instead of 12 base-36 characters. No operation handler needs to change; they all mint through that one routine.

Traces: servicecatalog.execute-deleted-plan-rejected, servicecatalog.execute-plan-already-executed-rejected, servicecatalog.execute-service-action-not-associated-with-artifact-rejected, servicecatalog.execute-service-action-unknown-provisioned-product-rejected (all four fail identically at their `_id(product, "prod")` setup assertion). Note: no open issue covers this — the open issue 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from.md concerns CreateProduct template validation and error wording, not id shape, and 20261121T224003Z-servicecatalog-describerecord-omits-the-aws-servicecatalog-record-tags-and.md concerns DescribeRecord.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replay against the built emulator: 14/29 passed; service heading `### servicecatalog` 9/16 (56%), 7 failed. Four of those failures are verdict=mismatch with neither an ALREADY-FILED stub marker nor an OWNED-by marker, i.e. net-new, and all four carry the identical assertion:

  [mismatch] servicecatalog.execute-deleted-plan-rejected
    File "check_servicecatalog_execute_deleted_plan_rejected.py", line 132, in _catalog -> _id(product, "prod")
    AssertionError: expected an id matching prod-[a-z2-7]{13}, got a str of length 17

  [mismatch] servicecatalog.execute-plan-already-executed-rejected
    File "check_servicecatalog_execute_plan_already_executed_rejected.py", line 132, in _catalog -> _id(product, "prod")
    AssertionError: expected an id matching prod-[a-z2-7]{13}, got a str of length 17

  [mismatch] servicecatalog.execute-service-action-not-associated-with-artifact-rejected
    File "check_servicecatalog_execute_service_action_not_associated_with_artifact_rejected.py", line 185, in run -> _id(product, "prod")
    AssertionError: expected an id matching prod-[a-z2-7]{13}, got a str of length 17

  [mismatch] servicecatalog.execute-service-action-unknown-provisioned-product-rejected
    File "check_servicecatalog_execute_service_action_unknown_provisioned_product_rejected.py", line 186, in run -> _id(product, "prod")
    AssertionError: expected an id matching prod-[a-z2-7]{13}, got a str of length 17

The checks' own helper states the rule: `def _id(value, prefix): """value, once it has the catalog id shape: a type prefix, a dash, 13 base-32 characters."""` with `re.fullmatch(prefix + r"-[a-z2-7]{13}", value)`.

The value asserted is produced by CreateProduct: `made["ProductViewDetail"]["ProductViewSummary"]["ProductId"]`.

Source of the divergence in the checkout:
  services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/state.rs:2468-2474 — doc comment "One id of the form `<prefix>-<12 lowercase base-36 characters>`" and `fn mint_id(&mut self, prefix: &str) -> String { ...; format!("{prefix}-{}", base36_12(scramble(self.next_id_seed))) }`
  services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/state.rs:2369-2376 — `pub fn next_product_id(&mut self)` calls `self.mint_id("prod")`; the sibling `next_product_view_id` (2380), `next_constraint_id` (2359) and the other `next_*_id` methods all mint the same way.
  services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/src/ops/create_product.rs:277 — `let id = state.next_product_id();`, the id returned in ProductViewSummary.ProductId.

Model evidence (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/smithy-model/com-amazonaws-servicecatalog-aws242servicecatalogservice.json): com.amazonaws.servicecatalog#Id = string, @length 1..100, @pattern ^[a-zA-Z0-9_\-]*$; ProductViewSummary.ProductId and .Id both target Id. The model therefore does NOT constrain the shape — the 13 base-32 characters are recorded AWS behavior, which the emulator's 12 base-36 characters violate on both length and alphabet.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from it (servicecatalog is bootstrapped: service-com-amazonaws-servicecatalog-aws242servicecatalogservice is in get_bootstrap_status().bootstrapped, and pending is empty).
2. Call CreateProduct with Name, Owner='sim2real', ProductType='CLOUD_FORMATION_TEMPLATE', a fresh IdempotencyToken and ProvisioningArtifactParameters={'Name':'first','Type':'CLOUD_FORMATION_TEMPLATE','Info':{'LoadTemplateFromURL': <any http(s) template URL>}}.
3. Read ProductViewDetail.ProductViewSummary.ProductId from the response. Observe it is `prod-` followed by 12 characters (17 characters total) drawn from base-36 [a-z0-9], e.g. it can contain 0, 1, 8 or 9. AWS returns `prod-` followed by 13 characters from [a-z2-7] (18 characters total), so `re.fullmatch(r'prod-[a-z2-7]{13}', product_id)` is None against the emulator and matches against AWS.
4. The same holds for every other minted id: ProvisioningArtifactDetail.Id (`pa-`), ProductViewSummary.Id (`prodview-`), CreatePortfolio's PortfolioDetail.Id (`port-`), CreateConstraint's ConstraintDetail.ConstraintId (`cons-`), CreateProvisionedProductPlan's PlanId (`plan-`) and ProvisionProductId (`pp-`), and a RecordDetail.RecordId (`rec-`) — all are minted by the one `mint_id` helper.
5. Re-replay the behavioral checks servicecatalog.execute-deleted-plan-rejected, servicecatalog.execute-plan-already-executed-rejected, servicecatalog.execute-service-action-not-associated-with-artifact-rejected and servicecatalog.execute-service-action-unknown-provisioned-product-rejected to confirm a fix; each currently aborts in its catalog-setup step on the `prod` id assertion before reaching the plan / service-action behavior it is written to check.
