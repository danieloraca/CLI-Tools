# Contacts CLI review log

User scope: implement `docs/implementation-plan.md`, independently review and fix each step, and make separate local commits. No remote publishing.

Baseline: `64cd10c`.

Reviewer reports are preserved below separately from owner dispositions. Each review identifies its frozen commit/diff or plan digest. The owner applies fixes; the reviewer remains read-only.

## Step 0 — plan review, pass 1

Reviewer: `/root/codebase_review` (independent reviewer role). Target: plan SHA-256 `a3e0635e2cf14d610cfe5d08475603b694671f8625aa602d2102e43faf8261ef`, code baseline `64cd10c`. Read-only source inspection.

Original reviewer report:

> Required — PLAN-01: Specify exclusive locking across apply and cleanup. Cleanup could observe cleared pending state while apply still owns its run lock, then race with subsequent writes. Require cleanup to acquire the existing apply lock before checking state and retain it through deletion and lifecycle updates. New batch journals also need exclusive locking. Include a focused concurrent-operation rejection check.
> Observation: Event registration can succeed with a different status, such as waitlisted instead of registered. Step 4 should report and checkpoint the actual outcome.
> Full planning depth and verification scope are proportionate. Existing-journal provenance limitations are adequately covered by the refusal/retention rule.

Owner disposition: PLAN-01 accepted (concurrency/data-integrity defect in proposed workflow). Added shared apply/cleanup locking, batch locks and concurrency checks. Accepted the attendance observation as a contract clarification; added readback/reporting of actual status. No implementation scope removed.

## Step 0 — plan review, pass 2

Target: plan SHA-256 `ea33ff7d277ac99f444abd276f5bbfafb35ace8b72b317f6ab697cad7fe996d8`, baseline `64cd10c`.

Original reviewer report:

> PLAN-01 resolved: exclusive lifecycle locking, shared apply/cleanup locking and concurrent-operation rejection tests are explicit. Attendance clarification resolved: actual status must be read back and recorded. No unresolved findings. Read-only inspection; no live operations.

Owner disposition: accepted. Plan review complete; implementation begins.

## Step 1 — source review, pass 1

Target: baseline `4d70704`, source/README manifest SHA-256 `a5a1c87658386f7f08239e7d2037098089cdc49213e15244f5b77608959611f7` (includes new gecko/query/catalog modules).

Original reviewer report:

> P2 QUERY-01: Floats and booleans are accepted by the CLI but Gecko equality only adds predicates for arrays, strings and integers. Equality with 0/"0"/false also enters the missing-value branch. Normalize supported comparisons or reject them. A recording PHP probe reproduced missing predicates.
> P2 QUERY-02: Gecko removes unknown/deleted/foreign IDs from a combined label condition, broadening a mixed valid/invalid request. Validate all IDs or use independent label conditions. Repeated identical IDs alone are handled correctly by Gecko.
> No further supported findings in query contracts, paging, JSON privacy or authenticated-client extraction. No live operations.

Owner disposition: accepted both. Decimal equality is encoded as text; boolean and zero/empty equality are rejected. Labels become separate AND conditions, so an absent label yields no matches. Added focused regressions and README limits. Initial checks: 73 tests and strict Clippy passed; read-only staging smoke test stopped at expired-token HTTP 401 before contact search.

## Step 1 — source review, pass 2

Target manifest SHA-256 `45b9a529b4e9b7a6323a950806c75c2736909a97dfbb2ac4410d6ef0c3f476a4`.

Original reviewer report:

> QUERY-02 resolved. QUERY-01 partially resolved: decimal contains still reaches Gecko's LIKE helper as a float, causing its predicate to be omitted. Normalize decimal contains too and cover it.

Owner disposition: accepted. Normalize every decimal scalar to text, including contains, and add its regression.

## Step 1 — source review, pass 3

Target manifest SHA-256 `6f7d26fe9f42c8d4c775fb1efaef768b2ab32cc9e0282bb5c65bf71dca9ccb7e`.

Original reviewer report:

> QUERY-01 resolved: decimal contains values now normalize to text, with regression coverage. No unresolved findings. Verified manifest/file hashes; read-only inspection.

Owner disposition: accepted. Step 1 complete with 74 passing tests, formatting and strict Clippy.
