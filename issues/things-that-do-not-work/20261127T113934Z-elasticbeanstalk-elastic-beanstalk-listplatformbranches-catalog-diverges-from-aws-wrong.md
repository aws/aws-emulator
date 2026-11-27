Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-catalog-list-platform-branches
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-catalog-list-platform-branches
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member[*]
Issue-Commit: 50a80390bb1e2ddab0fc937c65e59ad0dcd543f4

# Elastic Beanstalk ListPlatformBranches catalog diverges from AWS: wrong branch membership, order and BranchOrder

> _Note: this issue was identified against the code base at commit `50a80390bb1e2ddab0fc937c65e59ad0dcd543f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlatformBranches

## Description
ListPlatformBranches answers 200 with a PlatformBranchSummaryList that is not the catalog AWS returns. Five net-new divergence clusters are bundled here because they share one root fault: the emulator serves ListPlatformBranches out of a hand-built platform-branch table whose per-platform branch set, branch names, BranchOrder ordinals and list order were not taken from AWS's catalog, so every platform the corpus lists comes back wrong in the same way. All five causes MUST be closed before this issue is submitted.

Cause 1 of 5: BranchOrder ordinals are wrong for the Java platform's retired branches, which also reorders the list.
AWS: Smithy (elasticbeanstalk, ListPlatformBranches -> ListPlatformBranchesResult.PlatformBranchSummaryList of PlatformBranchSummary{PlatformName, BranchName, LifecycleState, BranchOrder, SupportedTierList}); BranchOrder is documented as "An ordinal number that designates the order in which platform branches have been added to a platform ... A larger BranchOrder value designates a newer platform branch within the platform." The model fixes the shape; which branches exist and which ordinal each carries is recorded behavior only. The recording shows AWS returning, for the Java platform, 'Java 8 running on 64bit Amazon Linux' with BranchOrder 8900 and 'Java 7 running on 64bit Amazon Linux' with BranchOrder 8800, both LifecycleState 'Retired', SupportedTierList ['WebServer/Standard','Worker/SQS/HTTP'].
Emulator: the same two entries come back with BranchOrder 9800 and 9700 respectively, so the entries AWS places at PlatformBranchSummaryList.member[10] and member[11] are reported with different ordinals (each position is simultaneously 'missing' the AWS entry and 'extra' with the emulator's). Status is 200 on both sides, so no error code or message is involved.
Why: BranchOrder is the documented way a client sorts branches by recency and picks the newest branch of a platform. With the ordinals shifted, a tool that sorts on BranchOrder (the EB CLI's platform picker, any console-style 'latest branch' selection) ranks branches in an order AWS does not report and can select a different branch than the caller would get against AWS.
Emulator component is shared with the other causes; see Fix below.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#6.

Cause 2 of 5: the PHP platform's branch list is short - AWS's last entries have no counterpart at all.
AWS: same model grounding; recorded behavior only for membership. For the PHP platform AWS returns, among others, 'Legacy - PHP 5.3 running on 64bit Amazon Linux' (BranchOrder 8900) and 'Legacy - PHP 5.3 running on 32bit Amazon Linux' (BranchOrder 8800), both LifecycleState 'Retired', SupportedTierList 'WebServer/Standard'.
Emulator: at the positions where AWS reports those two entries the emulator returns nothing (expected the two summaries, got None) - the served list simply ends earlier than AWS's. Status 200 on both sides.
Why: a client enumerating the branches of a platform concludes those branches do not exist. Code that resolves a branch name a user supplied (or that an existing environment/config template references) fails to find it and reports an unsupported platform, where AWS resolves it.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#8.

Cause 3 of 5: the Python platform's list is offset by one branch and truncated.
AWS: same model grounding; recorded behavior only. AWS reports 'Python 2.7 running on 32bit Amazon Linux' (BranchOrder 9600), 'Python 2.6 running on 32bit Amazon Linux' (BranchOrder 9500), 'Preconfigured Docker - Python 3.4 running on 64bit Debian' (8500) and 'Preconfigured Docker - Python 3.4 running on 64bit Amazon Linux' (8400) at consecutive positions.
Emulator: at those positions it reports the previous branch of its own table instead - 'Python 3.4 running on 32bit Amazon Linux' where AWS reports Python 2.7, 'Python 2.7 ...' where AWS reports Python 2.6, 'Python 2.6 ...' (BranchOrder 9400) where AWS reports the Preconfigured Docker branch - so from that point the list is shifted by one entry and the Preconfigured Docker branches are absent. Status 200 on both sides.
Why: positional readers get the wrong branch for an index; set-based readers lose the Preconfigured Docker branches entirely and see BranchOrder values attached to the wrong branch names.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#10.

Cause 4 of 5: the Ruby platform's branch names and ordinals are both wrong.
AWS: same model grounding; recorded behavior only. AWS reports 'Passenger with Ruby 2.6 running on 64bit Amazon Linux' (BranchOrder 8400), 'Passenger with Ruby 2.5 ...' (8300), 'Passenger with Ruby 2.4 ...' (8200) and the following 'Passenger with Ruby ...' branches, all LifecycleState 'Retired'.
Emulator: at those positions it reports 'Passenger Standalone with Ruby 2.6 running on 64bit Amazon Linux' with BranchOrder 9300, 'Passenger Standalone with Ruby 2.5 ...' with 9200, 'Passenger Standalone with Ruby 2.4 ...' with 9100 - a different BranchName spelling and a different ordinal for each of eleven consecutive entries (member[18] onward). Status 200 on both sides.
Why: BranchName is the identifier a caller passes back to Elastic Beanstalk (platform-branch filters, solution-stack/branch resolution). A branch name the emulator invents is not accepted by anything that was written against AWS's catalog, and vice versa: the AWS branch name the caller holds is not in the served list.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#11.

Cause 5 of 5: the Tomcat platform's newest branches are absent and (BETA) entries appear where AWS reports supported ones.
AWS: same model grounding; recorded behavior only. AWS reports 'Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2' (BranchOrder 11500), 'Tomcat 9 with Corretto 8 running on 64bit Amazon Linux 2' (11400), 'Tomcat 8.5 with Corretto 11 running on 64bit Amazon Linux 2' (11300) at the head of the retired-branch run (member[8] onward).
Emulator: it reports '(BETA) Tomcat 8.5 with Corretto 11 running on 64bit Amazon Linux 2' (BranchOrder 11100) and 'Tomcat 8.5 with Corretto 11 ...' (11100) at those positions and never reports the Tomcat 9 branches at all; the divergence runs over member[8..25]. Status 200 on both sides.
Why: the Tomcat 9 branches a client would select for a new environment are not in the catalog, and (BETA)-prefixed names AWS does not return are, so branch selection and any 'is this branch available' check disagree with AWS.

Fix: the ListPlatformBranches handler and the platform-branch catalog it reads (the static platform/branch table in the elasticbeanstalk service crate), plus the ordering applied by its serializer. The served list must be AWS's branch set per platform - exact BranchName spellings, LifecycleState, SupportedTierList and BranchOrder ordinals - in AWS's order, including the entries the current table stops short of. The SearchFilters handling itself is not implicated: every cluster is a 200 whose body membership/order is wrong.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#12.

Not covered by an open issue: the replay marks all five clusters unowned (no stub_path, no owned_by). The nearest open issue, 20261124T160907Z-cloudformation-elasticbeanstalk-elastic-beanstalk-option-catalog-omits-aws-autoscaling-launchconfiguration.md, is about the configuration-option catalog, not the platform-branch catalog, and 20261121T081235Z-elasticbeanstalk-elastic-beanstalk-describeconfigurationsettings-returns-an-option-list-whose.md owns DescribeConfigurationSettings only.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 19 fixtures, 157 steps, 11 mismatched, score 93.0%). All five clusters: service elasticbeanstalk, operation ListPlatformBranches, expected 200 / actual 200, verdict mismatch, owned_by [] and stub_path "".

Cause 1 - elasticbeanstalk-catalog-list-platform-branches#task#6:
  $.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList.member[10]: missing (expected {'LifecycleState': 'Retired', 'BranchOrder': '8900', 'SupportedTierList': {'member': ['WebServer/Standard', 'Worker/SQS/HTTP']}, 'BranchName': 'Java 8 running on 64bit Amazon Linux', 'PlatformName': 'Java'}, got {... 'BranchOrder': '9800' ...})
  $...member[11]: missing (expected BranchOrder '8800' for 'Java 7 running on 64bit Amazon Linux', got '9700'); the same two positions are also reported 'extra' with the emulator's entries.

Cause 2 - elasticbeanstalk-catalog-list-platform-branches#task#8:
  $...PlatformBranchSummaryList.member[24]: missing (expected {'LifecycleState': 'Retired', 'BranchOrder': '8900', 'SupportedTierList': {'member': 'WebServer/Standard'}, 'BranchName': 'Legacy - PHP 5.3 running on 64bit Amazon Linux', 'PlatformName': 'PHP'}, got None)
  $...member[25]: missing (expected 'Legacy - PHP 5.3 running on 32bit Amazon Linux', BranchOrder '8800', got None)

Cause 3 - elasticbeanstalk-catalog-list-platform-branches#task#10:
  $...member[12]: missing (expected 'Python 2.7 running on 32bit Amazon Linux' BranchOrder '9600', got 'Python 3.4 running on 32bit Amazon Linux' BranchOrder '9600')
  $...member[13]: missing (expected 'Python 2.6 ...' '9500', got 'Python 2.7 ...' '9500')
  $...member[14]: missing (expected 'Preconfigured Docker - Python 3.4 running on 64bit Debian' '8500', got 'Python 2.6 ...' '9400')
  $...member[15]: missing (expected 'Preconfigured Docker - Python 3.4 running on 64bit Amazon Linux' '8400', ...)

Cause 4 - elasticbeanstalk-catalog-list-platform-branches#task#11:
  $...member[18]: missing (expected 'Passenger with Ruby 2.6 running on 64bit Amazon Linux' BranchOrder '8400', got 'Passenger Standalone with Ruby 2.6 running on 64bit Amazon Linux' BranchOrder '9300')
  $...member[19]: missing (expected 'Passenger with Ruby 2.5 ...' '8300', got 'Passenger Standalone with Ruby 2.5 ...' '9200')
  $...member[20]: missing (expected 'Passenger with Ruby 2.4 ...' '8200', got 'Passenger Standalone with Ruby 2.4 ...' '9100')
  divergence_key spans member[18..28].

Cause 5 - elasticbeanstalk-catalog-list-platform-branches#task#12:
  $...member[8]: missing (expected 'Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2' BranchOrder '11500', got '(BETA) Tomcat 8.5 with Corretto 11 running on 64bit Amazon Linux 2' '11100')
  $...member[9]: missing (expected 'Tomcat 9 with Corretto 8 running on 64bit Amazon Linux 2' '11400', got 'Tomcat 8.5 with Corretto 11 ...' '11100')
  $...member[10]: missing (expected 'Tomcat 8.5 with Corretto 11 ...' '11300', got '(BETA) Tomcat 8.5 with Corretto 8 ...' '11000')
  divergence_key spans member[8..25].

Model grounding: Smithy elasticbeanstalk ListPlatformBranches -> ListPlatformBranchesResult{PlatformBranchSummaryList, NextToken}, paginated (inputToken NextToken / pageSize MaxRecords); PlatformBranchSummary{PlatformName, BranchName, LifecycleState (beta|supported|deprecated|retired), BranchOrder (integer, default 0, 'a larger BranchOrder value designates a newer platform branch'), SupportedTierList (WebServer/Standard|Worker/SQS/HTTP)}. Which branches exist per platform, their exact names and their ordinals are not modeled - recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Replay fixture elasticbeanstalk-catalog-list-platform-branches and diff the ListPlatformBranches response bodies at steps task#6 (Java), task#8 (PHP), task#10 (Python), task#11 (Ruby) and task#12 (Tomcat) against the recorded responses.
4. For each step, compare PlatformBranchSummaryList element-by-element: task#6 differs in BranchOrder on the Java retired branches; task#8's served list ends before AWS's last two PHP entries (got None); task#10 is shifted by one entry from member[12] on and drops the Preconfigured Docker Python branches; task#11 reports 'Passenger Standalone with Ruby <v>' names and 9xxx ordinals where AWS reports 'Passenger with Ruby <v>' and 8xxx; task#12 reports (BETA) Tomcat 8.5 entries where AWS reports Tomcat 9 with Corretto 11/8.
5. All five steps answer HTTP 200 on both sides - the divergence is entirely list membership, element order and BranchOrder/BranchName values in the body.
