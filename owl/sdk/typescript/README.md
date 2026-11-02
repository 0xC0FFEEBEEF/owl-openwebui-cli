# Owl SDK

Embed the owl coding agent in your workflows and apps.

The TypeScript SDK wraps the `owl` binary (installed with `owl/install.sh`, or built with `cargo build` in `codex-rs/`). It spawns `owl exec --experimental-json` and exchanges JSONL events over stdin/stdout.

## Quickstart

```typescript
import { Owl } from "owl-sdk";

const owl = new Owl();
const thread = owl.startThread();
const turn = await thread.run("Diagnose the test failure and propose a fix");

console.log(turn.finalResponse);
console.log(turn.items);
```

By default the SDK talks to the Open WebUI server configured in `~/.owl/config.toml` (or overrides it - see [Connecting to your server](#connecting-to-your-server)). The `owl` binary is found on `PATH`; set `owlPathOverride` to point at a specific build.

Call `run()` repeatedly on the same `Thread` instance to continue that conversation.

```typescript
const nextTurn = await thread.run("Implement the fix");
```

### Streaming responses

`run()` buffers events until the turn finishes. To react to intermediate progress - tool calls, streaming responses, and file change notifications - use `runStreamed()` instead, which returns an async generator of structured events.

```typescript
const { events } = await thread.runStreamed("Diagnose the test failure and propose a fix");

for await (const event of events) {
  switch (event.type) {
    case "item.completed":
      console.log("item", event.item);
      break;
    case "turn.completed":
      // owl's chat completions wire does not report token usage; `usage` is null.
      console.log("turn done", event.usage);
      break;
  }
}
```

### Connecting to your server

The SDK talks to your Open WebUI server through the standard owl settings:

```typescript
const owl = new Owl({
  baseUrl: "http://localhost:8080/api/v1", // sets OPENWEBUI_BASE_URL
  apiKey: "your-token", // sets OPENWEBUI_API_KEY (optional)
});
```

Omit `baseUrl`/`apiKey` and owl uses whatever is saved in `~/.owl/config.toml`. Models come from your server's own model listing; pin one with `startThread({ model: "..." })`.

Note: owl runs on the chat completions wire, so structured-output requests (`outputSchema`) are not supported - owl would reject them with "output_schema is not supported for Chat Completions API". Ask for JSON in the prompt instead.

### Attaching images

Provide structured input entries when you need to include images alongside text. Text entries are concatenated into the final prompt while image entries are passed to the owl CLI via `--image`.

```typescript
const turn = await thread.run([
  { type: "text", text: "Describe these screenshots" },
  { type: "local_image", path: "./ui.png" },
  { type: "local_image", path: "./diagram.jpg" },
]);
```

### Resuming an existing thread

Threads are persisted in `~/.owl/sessions`. If you lose the in-memory `Thread` object, reconstruct it with `resumeThread()` and keep going.

```typescript
const savedThreadId = process.env.OWL_THREAD_ID!;
const thread = owl.resumeThread(savedThreadId);
await thread.run("Implement the fix");
```

### Working directory controls

owl runs in the current working directory by default. To avoid unrecoverable errors, owl requires the working directory to be a Git repository. You can skip the Git repository check by passing the `skipGitRepoCheck` option when creating a thread.

```typescript
const thread = owl.startThread({
  workingDirectory: "/path/to/project",
  skipGitRepoCheck: true,
});
```

### Web search

`webSearchEnabled: true` (or `webSearchMode: "live" | "cached" | "disabled"`) offers the model
owl's `web_search` tool. Owl runs each search itself - the model's server does not need any
hosted search feature. Owl needs a search endpoint to query: set the `[search]` table in
`~/.owl/config.toml` (see [docs/config.md](../../docs/config.md#web-search)), or pass one per
thread with `webSearchEndpoint`:

```typescript
const thread = owl.startThread({
  webSearchEnabled: true,
  webSearchEndpoint: "http://search.internal:8888/search",
});
```

Without either, owl reports a configuration error up front instead of failing mid-turn.

### Controlling the owl CLI environment

By default, the owl CLI inherits the Node.js process environment. Provide the optional `env` parameter when instantiating the
`Owl` client to fully control which variables the CLI receive - useful for sandboxed hosts like Electron apps. Keep `HOME` (and `PATH`) in that map: owl resolves its settings home (`~/.owl`) from `HOME`.

```typescript
const owl = new Owl({
  env: {
    HOME: process.env.HOME!,
    PATH: "/usr/local/bin",
  },
});
```

The SDK still injects its required variables (such as `OPENWEBUI_BASE_URL` and `OPENWEBUI_API_KEY`) on top of the environment you provide.

### Development

```bash
pnpm install    # from the repository root (pnpm workspace)
pnpm --filter owl-sdk run build
pnpm --filter owl-sdk run test
pnpm --filter owl-sdk run lint
```

The tests spawn the real `owl` binary (`codex-rs/target/debug/owl`, built with `cargo build -p codex-cli`) against a local stand-in Open WebUI server. Set `OWL_HOME` to a scratch directory when running them so they don't touch your real settings.
