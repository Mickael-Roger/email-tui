# Security and privacy boundaries

## Local store and search

Real mailbox data uses the pinned io-pimdir public reader and producer APIs.
MailTUI does not acquire the owner role or issue private upstream SQL/schema
changes. Public enqueue operations respect declared source capabilities.
MIME is durably staged and pinned before its submit action is committed.
Neverest owns applying the queue, network connectors and reconciliation.
Mail-server credentials stay in Neverest's configuration. Carillon handles incoming data;
MailTUI observes public pimdir change cursors every two seconds. Explicit sends
start Neverest with `sync -a ACCOUNT --intents-only`, using an argument array,
null stdio, a timeout and cancellation on exit. Other writes wait for the
external consumer. The browser command opens the local HTML viewer.

Search uses MailTUI's separate **persistent SQLite FTS5 database** under
`$XDG_DATA_HOME/mailtui/` (default `~/.local/share/mailtui/`). It stores decoded
local message text, metadata and account/folder/message pointers. Newly created
index files use mode 0600 and their new application directory 0700 on Unix.
WAL permits querying while a separate worker updates the index. Demo indexes
are memory-only. No search request contacts IMAP or any mailbox server.

SQL values are parameterized, operator identifiers are whitelisted and FTS text
is quoted. No query/body log is emitted, but the index itself contains email
data and is part of the user's local application data and backups.
Original store creation and permissions remain the external owner's concern.

Raw MIME reads are capped at 16 MiB using a bounded stream. Parsed messages
over 512 parts are rejected. MIME parsing is delegated to mail-parser; this
is not a complete hostile-input sandbox or fuzzing certification. Terminal
control characters and bidi override/isolate controls are removed before
displaying untrusted text. Ordinary Unicode text is retained. Received attachments
can be saved explicitly from the reader to a chosen directory. Export writes
decoded bytes in the worker, strips path components from filenames and uses
exclusive creation plus numbered suffixes instead of overwriting existing files
or following filename symlinks. Newly exported files use 0600 on Unix. Selected outgoing files
are read in the blocking worker and included in the queued MIME, bounded to
16 MiB after encoding. Paths are never interpolated into a shell command.

## Optional LLM assistant

The assistant is disabled by default. Configuring it does not start requests:
only an explicit question sends the attached message, conversation and bounded
mail-tool results to the selected provider. API keys are read lazily from the
named environment variable; they are not stored in TOML or conversation history.
HTTP redirects are disabled. Mail tools enforce the fixed conversation account
scope, including recursive RFC thread traversal. Search/read tools cannot write;
reply/move tools produce proposals that the user applies through existing paths.
Mail content is identified as document data, not assistant instructions.

Codex uses its app-server OAuth flow and refresh management, with a dedicated
CODEX_HOME by default. No OAuth token is read by MailTUI or reused as an API key.
An explicitly shared codex_home shares login/logout state with its other users.
Threads are ephemeral, have an isolated temporary working directory, read-only
sandbox, no approvals, and shell/apply-patch/multi-agent/web-search tools disabled.
Only the MailTUI dynamic tool requests are handled by the client. Codex's own
configuration and provider-side retention are owned by Codex/the provider.
Conversations are memory-only in MailTUI. Request time, tool rounds, stream size,
body excerpts and conversation context are bounded. Cancellation drops the HTTP
request or terminates the Codex process; closing the dialog alone preserves it.

## Browser viewer

* Lazily binds **only** `127.0.0.1`, on an ephemeral port.
* Each session has a 256-bit random bearer token generated from the
  OS-seeded thread RNG. Token lookup uses an in-memory hash map, with no
  account IDs or subjects in the URL. Guessing a token is not a supported
  access path. Session URLs must nevertheless be treated as secrets.
* Expected literal `Host: 127.0.0.1:<port>` is required on message/CID routes;
  no forwarded-host trust or CORS privilege is granted. `/healthz` contains
  only a constant diagnostic response.
* At most eight sessions; oldest is evicted at the limit. TTL defaults to
  600 seconds. Expiration is enforced on every request and periodically
  purged. All sessions are revoked on shutdown.
* Ammonia strips active HTML and event handlers. Original inline styles,
  forms, scripts, iframes and SVG are not retained. Remote images start blocked.
  HTML-to-text conversion also sanitizes HTML first.
* Initially, `img[src]` permits only resolved CID references to authenticated
  same-message routes. Allowed content types are PNG/JPEG/GIF/WebP,
  maximum 2 MiB each. Unknown references are removed; duplicate CIDs choose
  the first matching MIME part. Original CSS is stripped, including URLs.
* A strict default CSP independently disables scripts, network fetches, forms,
  frames, plugins, fonts and embedding, allows same-origin images and only
  the viewer's inline CSS. The sandbox allows explicit external navigation.
* h opens the blocked variant, H the remote-image-enabled variant. The
  viewer-owned “Afficher les images distantes” button navigates to an
  authenticated, equally expiring variant of the same message. That variant
  keeps HTTP(S) image URLs and permits them in `img-src`; other active content
  remains disabled. “Masquer” returns to the blocked variant. These requests
  originate from the user's browser; there is no server-side remote proxy.
* External http(s) links remain clickable, with `noopener noreferrer`.
  Clicking one leaves the viewer and contacts that site explicitly.
* Responses use `Cache-Control: no-store`, `Referrer-Policy: no-referrer`,
  `X-Content-Type-Options: nosniff`, and inline content disposition.
  No request/access/token logging is installed.
* Sanitizing and constructing viewer documents runs in a blocking worker,
  not the terminal render loop. Browser commands are argument arrays, not
  shell interpolation; stdout/stderr are detached from the TUI.

The browser can retain history, page artifacts, screenshots or downloaded
data despite no-store headers. Revoking a session cannot retract content
already delivered. A process/browser extension running as the same user
may read memory or capability URLs. This viewer is not a hostile same-user
isolation boundary.

## SSH and subprocesses

SSH detection (SSH_CONNECTION/SSH_TTY) forces manual URL display. The URL
points to the remote machine; a local browser needs an explicit user-managed
loopback port forward. No tunnel or public listener is created. Running in
tmux does not change viewer routing. Browser launch success means only that
the command was started, not that Firefox confirmed display.

Neverest is invoked with argument arrays, never shell interpolation. Its
stdio is detached from the TUI, accounts run in parallel with one job per account, and child
processes are killed on timeout/shutdown. Per-account status distinguishes
successful, incomplete and conflicted runs. The TUI composer uses mail-builder
and queues delivery through Neverest rather than handling SMTP credentials.
An enqueue is not a delivery acknowledgement. No external editor or attachment
exporter is launched.
