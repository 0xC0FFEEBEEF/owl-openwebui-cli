# Authentication

owl has no account sign-in and no ChatGPT login. You connect to your own Open WebUI server:

- On first run, owl asks for the server URL and (optionally) an API token.
- They are saved to `~/.owl/config.toml` under `[model_providers.openwebui]` (`base_url`, `experimental_bearer_token`).
- You can edit them there directly, override the base URL with `OPENWEBUI_BASE_URL`, or point owl at a different settings home with `OWL_HOME`.

See [README-openwebui.md](../README-openwebui.md) for the full walkthrough.
