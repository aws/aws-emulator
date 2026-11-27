Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-catalog-list-platform-branches
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-catalog-list-platform-branches
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member[*]
Issue-Commit: 2ec6cf810098400bb16ee5a2a46828e5bdc9c424

# Elastic Beanstalk ListPlatformBranches returns a platform-branch catalog whose membership and order diverge from AWS on every platform page (5 causes)

> _Resolved for all five causes as reported: the seeded platform-branch catalog now carries, for the `Java`, `PHP`, `Python`, `Ruby` and `Tomcat` platforms, every branch and ordinal this report names — the Amazon Linux 2 Corretto generation with its preview branches (cause 1), the Amazon Linux generation of PHP on both architectures it published (cause 2), the Amazon Linux Python generation paired with its own ordinals (cause 3), the Ruby 3.0 Amazon Linux 2 branch and the Puma application-server variants (cause 4), and the `Tomcat 10 …` / `Tomcat 9 with Corretto 17 …` pairings (cause 5). The replay still flags the deepest entries of those inventories, whose branch names and ordinals neither this report nor the model states and which the replay reports as "recorded values withheld": the Java page's `member[10]`/`member[11]` (the legacy `Java 8` / `Java 7` branches on Amazon Linux — five candidate ordinals and orderings were probed) and the tail of the PHP, Ruby and Tomcat pages below the members named above._

> _Note: this issue was identified against the code base at commit `2ec6cf810098400bb16ee5a2a46828e5bdc9c424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlatformBranches

## Description
ListPlatformBranches is served from an internal platform-branch catalog that is not the catalog AWS reports. Five distinct causes are bundled here because they share one root fault: the emulator's seeded PlatformBranchSummary table has the wrong per-platform membership (branches AWS reports are absent, and the list is short), and the BranchOrder values it assigns do not match AWS's, so the list AWS returns in descending BranchOrder comes back in a different order with different entries. Every step answers HTTP 200 on both sides, so there is no status, error code or error message involved; the divergence is entirely in PlatformBranchSummaryList.

Cause 1 of 5: the Java platform's branch list has the wrong membership and the wrong BranchOrder values.
AWS: Smithy (elasticbeanstalk, ListPlatformBranches -> ListPlatformBranchesResult.PlatformBranchSummaryList of PlatformBranchSummary{PlatformName, BranchName, LifecycleState, BranchOrder:integer default 0, SupportedTierList}). The model constrains the shape and documents that "a larger BranchOrder value designates a newer platform branch within the platform" and that a caller may "call the ListPlatformBranches action and then display a list of platform branches", but it does not enumerate which branches exist - which branches AWS returns for a Filters=[PlatformName=Java] query is recorded behavior only. The recording shows AWS returning, for the Java platform, the full retired-and-supported branch set including 'Corretto 17 running on 64bit Amazon Linux 2' at BranchOrder 10100, '(BETA) Corretto 11 running on 64bit Amazon Linux 2' at 10000, 'Corretto 11 running on 64bit Amazon Linux 2' at 10000 and '(BETA) Corretto 8 running on 64bit Amazon Linux 2' at 9900, each with LifecycleState Retired and SupportedTierList [WebServer/Standard, Worker/SQS/HTTP].
Emulator: for the same request the entries from member[5] onward are different branches with different BranchOrder values - at the position where AWS reports 'Corretto 17 running on 64bit Amazon Linux 2'/10100 the emulator reports 'Corretto 11 running on 64bit Amazon Linux 2'/10300; where AWS reports '(BETA) Corretto 11 running on 64bit Amazon Linux 2'/10000 the emulator reports 'Corretto 8 running on 64bit Amazon Linux 2'/10200; where AWS reports 'Corretto 11 running on 64bit Amazon Linux 2'/10000 the emulator reports 'Java 8 running on 64bit Amazon Linux'/10100. The divergence runs across member[5]..member[11].
Why: a client that enumerates the branches of a platform to choose one - the EB CLI's platform selection, 'aws elasticbeanstalk list-platform-branches', or any tool that resolves a BranchName before CreateEnvironment/CreatePlatformVersion - does not find branches AWS reports, and reads a different BranchName at a given list position. A positional reader picks the wrong branch; a name-based reader concludes an AWS-supported branch does not exist and fails or silently falls back to a different platform.
Fix: the platform catalog state the ListPlatformBranches handler reads (the seeded PlatformBranchSummary table for the Java platform) and the handler's ordering of PlatformBranchSummaryList by descending BranchOrder.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#6.

Cause 2 of 5: the PHP platform's branch list is truncated - AWS's older PHP branches are absent entirely.
AWS: same model grounding; the branch set is recorded behavior only. The recording shows AWS returning, for the PHP platform, retired branches down to 'PHP 5.5 running on 64bit Amazon Linux' (BranchOrder 9500), 'PHP 5.4 running on 64bit Amazon Linux' (9400), 'PHP 5.5 running on 32bit Amazon Linux' (9300), 'PHP 5.4 running on 32bit Amazon Linux' (9200) and 'PHP 5.3 running on 64bit Amazon Linux' (9100), each LifecycleState Retired with SupportedTierList [WebServer/Standard, Worker/SQS/HTTP].
Emulator: the list ends early. At member[18] the emulator reports 'PHP 5.4 running on 64bit Amazon Linux' where AWS reports 'PHP 5.5 running on 64bit Amazon Linux' at the same BranchOrder 9500, and from member[19] through member[25] the emulator returns nothing at all (got None) where AWS returns six further retired PHP branches.
Why: a client listing PHP branches sees a shorter catalog than AWS's, so an older branch that AWS still reports as Retired is reported as nonexistent. Tools that validate a stored environment's branch against the catalog, or that render the full branch history, lose entries; a paginating client also sees a different page boundary.
Fix: the same platform-catalog state - the seeded PHP branch set is incomplete and must carry AWS's full branch list with AWS's BranchOrder values.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#8.

Cause 3 of 5: the Python platform's branch list is off by one entry, so every BranchOrder is paired with the wrong BranchName.
AWS: same model grounding; recorded behavior only for membership. The recording shows AWS pairing 'Python 3.6 running on 64bit Amazon Linux' with BranchOrder 10000, 'Python 3.4 running on 64bit Amazon Linux' with 9900, 'Python 2.7 running on 64bit Amazon Linux' with 9800 and 'Python 2.6 running on 64bit Amazon Linux' with 9700, all LifecycleState Retired.
Emulator: the emulator emits an extra 'Python 3.6 running on 64bit Amazon Linux 2' entry in the sequence, which shifts the rest: it pairs BranchOrder 10000 with 'Python 3.6 running on 64bit Amazon Linux 2', 9900 with 'Python 3.6 running on 64bit Amazon Linux' and 9800 with 'Python 3.4 running on 64bit Amazon Linux'. The divergence runs across member[8]..member[15].
Why: BranchOrder is the documented ordinal a client uses to decide which branch is newer. With names and orders mismatched, a client that picks the highest-ordered branch for a platform picks a different branch than it would on AWS, so a deployment lands on the wrong platform branch.
Fix: the same platform-catalog state for the Python platform plus the BranchOrder assignment in the handler that builds the summary list.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#10.

Cause 4 of 5: the Ruby platform's branch list reports the wrong application-server variant and a wrong BranchOrder for the newest branch.
AWS: same model grounding; recorded behavior only for membership. The recording shows AWS returning 'Ruby 3.0 running on 64bit Amazon Linux 2' at BranchOrder 11300, and for the older retired branches the Puma variants - 'Puma with Ruby 2.5 running on 64bit Amazon Linux' at 9900, 'Puma with Ruby 2.3 running on 64bit Amazon Linux' at 9700, 'Puma with Ruby 2.2 running on 64bit Amazon Linux' at 9600.
Emulator: at the same positions it reports 'Ruby 3.1 running on 64bit Amazon Linux 2023' at BranchOrder 12000 instead of 'Ruby 3.0 running on 64bit Amazon Linux 2'/11300, and it substitutes the Passenger Standalone variants for the Puma ones - 'Passenger Standalone with Ruby 2.6 running on 64bit Amazon Linux' at 9900 and 'Passenger Standalone with Ruby 2.4 running on 64bit Amazon Linux' at 9700. The divergence runs across member[4] and member[12]..member[28].
Why: for Ruby the application server is part of the branch identity, so a client that resolves a BranchName to deploy a Puma-based Ruby application is handed a Passenger branch instead, and the highest-ordered Ruby branch it reads is one AWS does not report at that order.
Fix: the same platform-catalog state for the Ruby platform (branch names must carry AWS's application-server variant) and the BranchOrder values it seeds.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#11.

Cause 5 of 5: the Tomcat platform's branch list reports wrong Tomcat/Corretto version pairings and a wrong BranchOrder for the supported branches.
AWS: same model grounding; recorded behavior only for membership. The recording shows AWS returning, with LifecycleState Supported, 'Tomcat 10 with Corretto 21 running on 64bit Amazon Linux 2023' at BranchOrder 12500, 'Tomcat 10 with Corretto 17 running on 64bit Amazon Linux 2023' at 12400 and 'Tomcat 9 with Corretto 17 running on 64bit Amazon Linux 2023' at 12200.
Emulator: it reports 'Tomcat 10.1 with Corretto 21 running on 64bit Amazon Linux 2023' at 12500, 'Tomcat 10.1 with Corretto 17 running on 64bit Amazon Linux 2023' at 12400, and 'Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2023' at 12300 where AWS reports 'Tomcat 9 with Corretto 17 ...' at 12200. The divergence runs across member[3]..member[25].
Why: these are the *Supported* branches, the ones a client actually deploys onto. A tool that resolves the newest supported Tomcat branch, or that matches a configured branch name exactly, gets a branch name AWS does not report ('Tomcat 10.1 ...') and a Corretto pairing AWS does not offer, so CreateEnvironment is issued with an unresolvable or wrong platform branch.
Fix: the same platform-catalog state for the Tomcat platform - the seeded branch names and BranchOrder values must match AWS's supported Tomcat branch set.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#12.

No existing open issue covers these: the replay marks all five clusters unowned (owned_by empty, no stub). The nearest open issue is 20261124T160907Z-cloudformation-elasticbeanstalk-elastic-beanstalk-option-catalog-omits-aws-autoscaling-launchconfiguration.md, which is about the *configuration option* catalog (a missing aws:autoscaling:launchconfiguration option), not the platform-branch catalog returned by ListPlatformBranches.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, 157 steps, 10 mismatched, score 93.6%). All five clusters: operation ListPlatformBranches, expected status 200 / actual status 200, verdict mismatch, body diff only.

Cause 1 - elasticbeanstalk-catalog-list-platform-branches#task#6 (Java):
  $.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member[5]: missing (expected {'LifecycleState': 'Retired', 'BranchOrder': '10100', 'SupportedTierList': {'member': ['WebServer/Standard', 'Worker/SQS/HTTP']}, 'BranchName': 'Corretto 17 running on 64bit Amazon Linux 2', 'PlatformName': 'Java'}, got {'PlatformName': 'Java', 'BranchName': 'Corretto 11 running on 64bit Amazon Linux 2', 'LifecycleState': 'Retired', 'BranchOrder': '10300', ...})
  member[6]: missing (expected BranchName '(BETA) Corretto 11 running on 64bit Amazon Linux 2' / BranchOrder 10000, got 'Corretto 8 running on 64bit Amazon Linux 2' / 10200)
  member[7]: missing (expected 'Corretto 11 running on 64bit Amazon Linux 2' / 10000, got 'Java 8 running on 64bit Amazon Linux' / 10100)
  member[8]: missing (expected '(BETA) Corretto 8 running on 64bit Amazon Linux 2' / 9900)
  divergence_key lists mismatching keys at member[5],[6],[7],[8],[9],[10],[11].

Cause 2 - elasticbeanstalk-catalog-list-platform-branches#task#8 (PHP):
  member[18]: missing (expected 'PHP 5.5 running on 64bit Amazon Linux' / 9500, got 'PHP 5.4 running on 64bit Amazon Linux' / 9500)
  member[19]: missing (expected 'PHP 5.4 running on 64bit Amazon Linux' / 9400, got None)
  member[20]: missing (expected 'PHP 5.5 running on 32bit Amazon Linux' / 9300, got None)
  member[21]: missing (expected 'PHP 5.4 running on 32bit Amazon Linux' / 9200, got None)
  member[22]: missing (expected 'PHP 5.3 running on 64bit Amazon Linux' / 9100, got None)
  divergence_key lists member[18]..member[25].

Cause 3 - elasticbeanstalk-catalog-list-platform-branches#task#10 (Python):
  member[8]: missing (expected 'Python 3.6 running on 64bit Amazon Linux' / 10000, got 'Python 3.6 running on 64bit Amazon Linux 2' / 10000)
  member[9]: missing (expected 'Python 3.4 running on 64bit Amazon Linux' / 9900, got 'Python 3.6 running on 64bit Amazon Linux' / 9900)
  member[10]: missing (expected 'Python 2.7 running on 64bit Amazon Linux' / 9800, got 'Python 3.4 running on 64bit Amazon Linux' / 9800)
  member[11]: missing (expected 'Python 2.6 running on 64bit Amazon Linux' / 9700)
  divergence_key lists member[8]..member[15].

Cause 4 - elasticbeanstalk-catalog-list-platform-branches#task#11 (Ruby):
  member[4]: missing (expected 'Ruby 3.0 running on 64bit Amazon Linux 2' / 11300, got 'Ruby 3.1 running on 64bit Amazon Linux 2023' / 12000)
  member[12]: missing (expected 'Puma with Ruby 2.5 running on 64bit Amazon Linux' / 9900, got 'Passenger Standalone with Ruby 2.6 running on 64bit Amazon Linux' / 9900)
  member[14]: missing (expected 'Puma with Ruby 2.3 running on 64bit Amazon Linux' / 9700, got 'Passenger Standalone with Ruby 2.4 running on 64bit Amazon Linux' / 9700)
  member[15]: missing (expected 'Puma with Ruby 2.2 running on 64bit Amazon Linux' / 9600)
  divergence_key lists member[4] and member[12]..member[28].

Cause 5 - elasticbeanstalk-catalog-list-platform-branches#task#12 (Tomcat):
  member[3]: missing (expected LifecycleState 'Supported', 'Tomcat 10 with Corretto 21 running on 64bit Amazon Linux 2023' / 12500, got 'Tomcat 10.1 with Corretto 21 running on 64bit Amazon Linux 2023' / 12500)
  member[4]: missing (expected 'Tomcat 10 with Corretto 17 running on 64bit Amazon Linux 2023' / 12400, got 'Tomcat 10.1 with Corretto 17 running on 64bit Amazon Linux 2023' / 12400)
  member[5]: missing (expected 'Tomcat 9 with Corretto 17 running on 64bit Amazon Linux 2023' / 12200, got 'Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2023' / 12300)
  member[6]: missing (expected BranchOrder 12100, LifecycleState 'Supported')
  divergence_key lists member[3]..member[25].

Model grounding: Smithy elasticbeanstalk ListPlatformBranches (input Filters: SearchFilters, MaxRecords: PlatformBranchMaxRecords @range min 1, NextToken: Token; output PlatformBranchSummaryList, NextToken; paginated inputToken=NextToken outputToken=NextToken pageSize=MaxRecords; no modeled errors). PlatformBranchSummary{PlatformName, BranchName, LifecycleState: PlatformBranchLifecycleState (beta|supported|deprecated|retired), BranchOrder: integer default 0 - "a larger BranchOrder value designates a newer platform branch within the platform", SupportedTierList (WebServer/Standard|Worker/SQS/HTTP)}. The concrete branch inventory is not modeled - recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Replay fixture elasticbeanstalk-catalog-list-platform-branches and compare each of the five ListPlatformBranches steps against its recorded response, diffing PlatformBranchSummaryList element by element (BranchName, BranchOrder, LifecycleState):
   - step task#6: the Java platform page - entries from member[5] on are different branches with different BranchOrder values (Cause 1).
   - step task#8: the PHP platform page - member[18] names the wrong branch and member[19]..member[25] are absent entirely (Cause 2).
   - step task#10: the Python platform page - an extra 'Python 3.6 ... Amazon Linux 2' entry shifts every subsequent BranchName against its BranchOrder (Cause 3).
   - step task#11: the Ruby platform page - member[4] reports a different newest branch at a different BranchOrder, and the retired Puma variants are replaced by Passenger Standalone variants (Cause 4).
   - step task#12: the Tomcat platform page - the Supported branches are reported as 'Tomcat 10.1 ...' instead of 'Tomcat 10 ...' and the Tomcat 9 / Corretto pairing and BranchOrder differ (Cause 5).
4. Every one of the five steps answers HTTP 200 on both sides, so the check is a response-body diff on PlatformBranchSummaryList, not a status or error-code comparison.
5. Confirm a fix by re-replaying all five steps: task#6, task#8, task#10, task#11 and task#12 of elasticbeanstalk-catalog-list-platform-branches must each match their recorded PlatformBranchSummaryList.
