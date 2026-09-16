# CLI Tools

Rust terminal tools for logging in, choosing a profile, browsing contacts in a TUI, and building repeatable Gecko development scenarios. Requires Rust 1.89 or newer (dependencies may require a newer toolchain).

Blocking login, profile-loading, and contacts page changes show loading spinners while requests are in flight.

## Setup

From the repository directory, copy the environment template:

```sh
cp .env.example .env
```

Replace the placeholder URLs in `.env` with the account and app API base URLs for the same Gecko environment. Include any required API version path. The CLI loads `.env` automatically; `.env.example` is only a template and is not loaded. Git ignores your local `.env`.

| Environment variable | Command-line option | Used for |
| --- | --- | --- |
| `CLI_TOOLS_ACCOUNT_API_URL` | `--base-url` | Login, profile selection and app token claims |
| `CLI_TOOLS_APP_API_URL` | `--app-api-base-url` | The post-login menu, contact browsing and scenario application |

With `.env` configured, log in without passing the URLs:

```sh
cargo run -- login --email you@example.com
```

You can also pass the URLs explicitly. Replace these example URLs with your environment's values:

```sh
cargo run -- login \
  --base-url https://account-api.example.test \
  --app-api-base-url https://app-api.example.test \
  --email you@example.com
```

`--email` is optional: the CLI prompts for it when omitted. It also prompts for your password and any required MFA response.

If login reports that `--base-url <BASE_URL>` is required, `CLI_TOOLS_ACCOUNT_API_URL` was not found. Check that you copied [.env.example](.env.example) to `.env`, configured its values, and are running from the repository directory, or supply `--base-url` explicitly.

Saved tokens and sessions are replaced atomically. On Unix, new files are private (`0600`) and newly created configuration directories use `0700`; replacing an older token file also corrects its permissions. Output paths must be regular files rather than symlinks.

## Contacts

Open the saved-session contacts browser:

```sh
cargo run -- contacts
```

Search and filters run on Gecko and remain active when changing pages:

```sh
cargo run -- contacts --search 'example.test' --sort created-desc
cargo run -- contacts --where '28:empty' --label-id 9 --created-after 2026-01-01 --plain
cargo run -- filters list
cargo run -- contacts --saved-filter 12 --json
```

`--where FIELD_ID:OP[:VALUE]` supports `eq`, `ne`, `contains`, `gt`, `lt`, `empty` and `not-empty`. Use the actual contact field ID. Repeat conditions or label IDs to require all of them. Values are strings or numbers (quote inside the argument to keep numeric-looking text as text). Boolean comparisons and equality/inequality with zero or empty text are rejected because Gecko cannot evaluate them as stated; decimal equality is encoded as text. An unknown label makes the combined search match no contacts. Missing-value operators take no value. Dates accept RFC3339 or `YYYY-MM-DD` at midnight UTC; `--created-after` is inclusive and `--created-before` exclusive.

Saved filters combine with keyword search, but cannot combine with explicit field/label/date conditions because Gecko replaces the saved conditions in that combination. Unknown saved filter and field IDs fail before the contact search. `--sort` supports `id-asc` (default), `id-desc`, `name-asc`, `email-asc`, `created-asc`, `created-desc`, and `updated-desc`, with an ID tie-breaker. Page sizes are 1–500.

`--plain` prints the table; `--json` prints only a structured contacts/pagination object, with the same privacy masking as the TUI. Saved-filter discovery prints JSON. Both commands use the saved app token/session and accept their existing path overrides.

Controls:

- Move selection: `up`/`down` or `j`/`k`
- Change page: `left`/`right` or `h`/`l`
- Change page size: `+`/`-`, `[`/`]`, or `1`/`2`/`3` for `15`/`30`/`50`
- Open contact overview: `enter`
- Scroll contact overview: `up`/`down` or `j`/`k`; `PageUp`/`PageDown` for a page, `Home`/`End` for the beginning/end
- Return from contact overview: `b`, `left`, or `backspace`
- Quit: `q` or `esc`

The contact overview loads current profile values, shows a header with name/email/labels, and includes an activity placeholder for a later pass.

Contact dates display in UTC, including epoch and offset-bearing timestamps; API dates without a timezone are interpreted as UTC. Name, email and creation time use stable contact properties. Phone and last-chat values are resolved from the profile's configured list columns and are blank when those field types are absent from the list.

Sensitive fields are masked in the list, plain output and contact headings, including fields omitted from the configured list. Stable properties are also masked if their privacy metadata is unavailable.

Contact reads match the app token against the selected profile's `ExternalId` and `AccountId`. The account service's `ProfileId` identifies the selection; it is a different ID from the token's `profile` claim. The app API's `/auth/check` confirms the account and app user and resolves the numeric IDs used in Gecko's request and response headers.

If an older saved session reports a profile or app-user mismatch, reselect your profile to refresh its saved IDs and app tokens:

```sh
cargo run -- profiles
```

Use `cargo run -- login` if the saved login token has expired.

Profile selection, menu, and contacts screens show their controls in the bottom-right legend panel.

## Fields and exports

```sh
cargo run -- fields list
cargo run -- contacts --columns id,email,field:93 --json
cargo run -- contacts --search 'example.test' --csv --all --output contacts.csv
cargo run -- contacts --columns id,full_name,field:93 --plain
```

`fields list` prints IDs, labels, types, required/sensitive flags and available option metadata. Custom columns use `field:ID`, independent of the UI's six configured list columns. JSON preserves Gecko's typed `value` (numbers, booleans, objects, arrays and null); sensitive values remain masked. The `phone` and `last_chat_message` aliases also read actual field IDs, even outside the UI list configuration. An ambiguous field type requires an explicit `field:ID`; an absent type yields null. Built-in columns are `id`, `full_name`, `email`, `phone`, `created_at`, `last_chat_message` and `labels`.

Exports use the requested page unless `--all` is set. `--all` starts at page 1, continues through server page-size caps, checks for repeated IDs and stops on errors or changed result counts. Exports are buffered up to `--max-rows` (default 100,000, maximum 1,000,000); exceeding that limit is an error, so narrow the query or raise it explicitly. Results are not a server snapshot: run against stable development data for repeatable exports.

CSV quotes every cell, including embedded quotes, commas and newlines. Arrays/objects use JSON text inside a cell and null is an empty cell. Export flags select noninteractive output, defaulting to JSON unless `--csv` or `--plain` is supplied. No output is written until the full export succeeds. `--output` publishes a new private file atomically and never overwrites an existing file. Without it, completed output goes to stdout.

## Targeted batch changes

Discover the available resource IDs, then preview an exact selection:

```sh
cargo run -- labels list
cargo run -- consents list
cargo run -- batch label-add --label-id 9 --contact-id 101 --contact-id 102 \
  --profile-id YOUR_DEV_PROFILE_ID --journal labels.batch-state.json
```

The JSON preview names the target account/profile, operation and all selected IDs, with a per-contact `change_required` flag. Add `--execute` to the same command to perform it. The journal binds the exact operation and selection; change either by choosing a new journal.

Supported operations are `label-add`, `consent-grant` and `consent-revoke`. Consent commands take `--consent-id`; label addition takes `--label-id`. Label removal is not exposed: Gecko's mass-action permission map rejects that operation, and replacing the entire label list could overwrite another editor's changes. IDs must refer to existing resources in the selected account. Every remaining contact is checked before the first write. An already-satisfied operation is verified and skipped.

Use generated contacts as the selection instead of listing their IDs:

```sh
cargo run -- batch consent-grant --consent-id 7 \
  --scenario-state admissions.apply-state.json --profile-id YOUR_DEV_PROFILE_ID \
  --journal consents.batch-state.json
```

This selects the journal's recorded, fully populated contacts. Pending/partially populated contacts or a different target are rejected. It does not infer targets from email addresses, labels or queries. The scenario journal stays locked through the batch to prevent concurrent creation/cleanup.

Each write uses a single-contact Gecko add/remove action, so unrelated labels and consents are preserved. The result is read back before recording success. These are real Gecko actions: ordinary write hooks and workflows may run. A queued, failed or unconfirmed action remains `pending` and is never retried automatically. No shared staging records are changed by the CLI's test suite.

Keep the private batch journal. A completed rerun makes no API requests. On interruption, completed contacts stay recorded. For a pending contact, inspect Gecko after any queued action finishes: if the desired state is confirmed, add its ID to `completed` with `{"verified": true}` and clear `pending`; if the action definitely did not run, only clear `pending`. If uncertain, leave it pending. Rerun with the original operation and IDs. Stop other processes and back up the journal before manual reconciliation. `*.batch-state.json` and its lock are ignored by Git; keep custom journal names out of version control yourself.

## Test-scenario builder

Generate a scenario offline, without credentials or API requests:

```sh
cargo run -- scenario generate \
  "create a profile with 500 contacts, duplicate emails, custom fields, and restricted permissions" \
  --name duplicate-review --seed 42 --output duplicate-review.json
```

This produces 500 contacts with 450 distinct email addresses, three custom fields (`course`, `cohort`, `access_notes`), and a separate group that grants `contacts_view`. Names include accents, apostrophes and non-Latin characters; notes include multiple lines. All generated emails use `example.test`. A fixed seed and the same options produce byte-identical JSON, independent of the date and machine.

The quoted request uses a small, local grammar, with no AI service required. Start with `create a profile with N contacts`, `create a scenario with N contacts`, or `N contacts`, then add comma-separated clauses:

| Clause | Result |
| --- | --- |
| `duplicate emails` | 10% extra contacts reuse an email (minimum one) |
| `25 duplicate emails` | Exactly 25 extra contacts reuse an email |
| `missing emails` | 5% of contacts have no email (minimum one) |
| `10 missing emails` | Exactly 10 contacts have no email |
| `custom fields` | Course (text), cohort (number), access notes (textarea) |
| `restricted permissions` | A new group granting `contacts_view`, plus Gecko's mandatory permissions |

Unknown clauses are rejected. Every duplicate counts as an extra contact beyond the first occurrence, and missing emails are excluded from the duplicate count. At least one non-missing, unique email must remain when requesting duplicates. Up to 10,000 contacts and 20 custom fields are supported.

Use flags for precise counts and additional field definitions:

```sh
cargo run -- scenario generate --name admissions --seed 123 \
  --contacts 500 --duplicate-emails 50 --missing-emails 20 \
  --custom-field programme:text --custom-field entry_year:number \
  --custom-field support_notes:textarea --restricted-permissions \
  --output admissions.json
```

Counts passed as flags override counts in a quoted request. `--custom-fields` adds the three preset fields; repeat `--custom-field KEY:TYPE` to add others. Custom keys use lowercase letters, digits and underscores, start with a letter, and have at most 32 characters. `name` and `email` are reserved. Without `--output`, stdout contains only JSON, suitable for a pipe. An existing output file is never overwritten.

The versioned fixture is editable: contacts map logical field keys to values; missing values are `null`. The entire fixture is validated before application. Field order in JSON is immaterial. See [the small example](examples/scenarios/restricted-contacts.json).

Gecko's contact write API silently ignores numeric `0`, the string `"0"`, and empty strings in custom fields. This includes strings that become empty or `"0"` after trimming whitespace. Fixtures containing those values are rejected before any API request; use `null` for missing values.

### Apply to a development profile

This version populates an existing Gecko profile selected through the CLI. The phrase “create a profile” generates a local scenario specification; it does **not** provision a new Gecko account or account-auth profile.

Log in or select a development profile with the existing commands, using `--no-menu` when scripting. Then apply the reviewed fixture:

```sh
cargo run -- profiles --profile-id YOUR_DEV_PROFILE_ID --no-menu
cargo run -- scenario apply admissions.json --profile-id YOUR_DEV_PROFILE_ID
```

Apply uses `CLI_TOOLS_APP_API_URL` (or `--app-api-base-url`), the saved app token and selected session. Override their paths with `--app-token-file` and `--session-file`. The explicit profile ID must match the saved selection, while the token must match its app user and account. Before writes, `/auth/check` verifies those identities and resolves the numeric routing ID checked against each `Gecko-Account` response header; a missing or mismatched header stops the run. Local JWT decoding checks consistency only; Gecko verifies the token. Applying performs writes immediately after preflight; generation is the offline preview step.

The command resolves the profile's actual name and email field IDs, creates the custom fields with matching disabled, creates the requested group, and creates contacts. It requires one unambiguous name field and one email field. Existing required fields other than name/email, or a required email field with missing-email fixtures, cause a preflight error before writes.

Gecko automatically merges new contacts on matching fields. To preserve duplicates, each contact is created with a unique temporary name and, when needed, email, then updated by ID with its final fixture values. Missing emails are omitted at creation. This takes two requests per contact and requires contact creation and update permissions, plus field/group creation permissions when those features are requested. Apply on a development profile: ordinary Gecko write hooks and workflows still run, and temporary identities can appear in contact history.

The read-only group is created without assigning users. Assign a development test user to that group in Gecko to exercise restricted access. The logged-in user's permissions are not changed.

### Reruns and interrupted runs

Apply checkpoints successful field, group and contact IDs in `admissions.apply-state.json`. Keep this file: rerunning the same fixture against the same target skips completed operations; a fully completed run performs no API requests. A filesystem lock prevents two processes from using the same journal concurrently. Journals contain a SHA-256 fixture fingerprint and resource IDs, never tokens or contact data. Default journal/checkpoint filenames are ignored by Git.

Use `--state-file PATH` for a separate target/run. A journal refuses a changed fixture, API URL, account or profile. Each fresh journal represents a new run and can create another set of resources; deterministic generation does not make separate journals globally idempotent. Keep custom-named journals out of version control yourself.

Before every write the journal records `pending`. If a request fails, times out, returns no ID, or the process stops during the write, that operation is **not automatically retried**: Gecko may already have committed it. Successful earlier operations remain recorded.

To recover, stop other apply processes, back up the journal, and inspect the target in Gecko. The journal's `pending` key identifies the operation (`field:KEY`, `group:KEY`, `contact:KEY`, or `populated:KEY`). For a confirmed successful operation, add its resource ID as a string under that key in `completed`, then set `pending` to `null`. For a contact's final update, use the existing contact ID and mark it complete only after checking the final values. If the operation definitely did not happen, clear `pending` without adding a completed entry. If its outcome is unknown, retain `pending` until resolved. Rerun the same command after reconciliation.

If a crash leaves a `.tmp` checkpoint, compare it with the main journal and Gecko before promoting or removing it. Do not discard a journal simply to retry: that starts another run. Resource IDs are also available for manual cleanup in Gecko; automated deletion is not included.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Scenario tests cover deterministic output, exact duplicate/missing counts, fixture validation, real HTTP payload shapes, duplicate-preserving updates, profile selection, required-field preflight, checkpoint recovery and reruns against local mock HTTP servers. They require loopback sockets and do not contact a live Gecko environment.

The local delivery plan and review record are in [docs/implementation-plan.md](docs/implementation-plan.md) and [docs/review-log.md](docs/review-log.md).
