# Assistant implementation

Configuration remains TOML. See the executable examples and interaction guide
in [README](../README.md#assistant-llm-facultatif).

## Provider boundary

`llm::Assistant` owns a single cancellable task and a bounded event channel.
`llm::api` supports OpenAI Chat Completions and Responses tool loops with SSE.
Chat function deltas are assembled by index; Responses completion output items,
including reasoning items, are retained for subsequent function outputs within
the same turn. SSE decodes UTF-8 only after a complete line and supports CRLF,
comments, multiline data and arbitrary byte boundaries. A missing terminal
completion is an error, not success. HTTP credentials resolve only on a question.

`llm::codex` speaks newline-delimited app-server RPC over child stdio. It
initializes experimental API support, handles ChatGPT OAuth account operations,
starts ephemeral threads with the configured model and registers dynamic tools.
Server requests/notifications that precede a client RPC acknowledgement are queued.
App-server owns credentials and refresh; MailTUI does not parse auth.json. The
dynamic-tools protocol is experimental and requires a compatible recent Codex.
Codex-specific fields follow the OpenAI app-server v2 protocol (validated against
the rust-v0.154.0 schema and current dynamic-tools documentation).

## Tools and actions

Both providers call the same scoped, blocking mail adapter:

- `search_mail`: structured FTS/fuzzy search; 200 candidates, pages of 1–20.
- `read_mail`: decoded body excerpt (16,000 characters), recipients, RFC identity
  and attachment names, with a truncation indicator and character-offset pagination.
- `read_thread`: exact RFC-header connectivity in schema v3, at most 20 bodies
  of 3,000 characters, with bounded membership and explicit limit indicators.
- `find_related_mail`: separate subject heuristic, complemented by model-chosen
  participant/date/topic searches.
- `list_folders`: available account-scoped destinations, optional name filter and
  50-folder pagination.
- `propose_reply`: derive the recipient and reply headers from the source MIME;
  return a composer proposal, never send.
- `propose_move`: validate source references/account/destination and return a
  proposal, never mutate during inference.

Tool outputs identify exact placements through stable session source numbers.
The client enforces account scope independently of the prompt. Public reader
APIs resolve MIME; search uses the independent local SQLite index. The TUI
rechecks a move destination and sends an explicit application through the normal
write worker. Drafts reuse MIME generation and preserve In-Reply-To/References.

## Conversation lifecycle

One in-memory conversation survives dialog close/navigation. New/global resets
pin the current scope and invalidate old run IDs. Esc closes; `/stop` cancels;
the two operations are deliberately distinct. A later question includes the
human-readable conversation and known source references. Tool results can be
reread as needed; no hidden per-provider durable conversation is required.
There is no embedding index or extraction of document attachment contents.

Tests use actual local HTTP servers plus an executable fake Codex RPC peer.
Live OAuth login and model inference require the user's provider/account and are
not claimed as tested by those fixtures.
