# Changelog

Owl has no release feed: the change list is this repository's history (see
README-openwebui.md, "Where the fork's own history lives"). Highlights of this fork
relative to upstream `openai/codex` at tag `rust-v0.90.0`:

- The command is `owl` and its settings live in `~/.owl` (`OWL_HOME`).
- Built-in `openwebui` provider (chat completions at `/api/v1`) is the default and
  the only network built-in; the model list comes from your server.
- Removed from the codebase: the `openai` built-in provider, ChatGPT sign-in
  (`owl login` / `owl logout` and the TUI auth screens), `owl apply`, `owl cloud`,
  ChatGPT apps/connectors, the cloud-tasks crates, upstream's update check, and the
  built-in analytics exporter to `ab.chatgpt.com` (otel now exports nowhere by
  default).
- Little Owl ASCII mascot replaces the upstream welcome animation.
