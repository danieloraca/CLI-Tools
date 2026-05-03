# CLI Tools

Rust terminal tools for logging in, choosing a profile, and browsing contacts in a TUI.

Blocking login, profile-loading, and contacts page changes show loading spinners while requests are in flight.

## Setup

Create a local `.env` file:

```sh
CLI_TOOLS_ACCOUNT_API_URL='...'
CLI_TOOLS_APP_API_URL='...'
```

Run:

```sh
cargo run -- login --email you@example.com
```

## Contacts

Open the saved-session contacts browser:

```sh
cargo run -- contacts
```

Controls:

- Move selection: `up`/`down` or `j`/`k`
- Change page: `left`/`right` or `h`/`l`
- Change page size: `+`/`-`, `[`/`]`, or `1`/`2`/`3` for `15`/`30`/`50`
- Open contact overview: `enter`
- Return from contact overview: `b`, `left`, or `backspace`
- Quit: `q` or `esc`

The contact overview loads current profile values, shows a header with name/email/labels, and includes an activity placeholder for a later pass.

Profile selection, menu, and contacts screens show their controls in the bottom-right legend panel.
