# Sandboxing and approvals

owl runs model-requested shell commands inside your OS sandbox. In interactive mode the default policy allows writes inside your project directory and asks for approval for anything beyond it; `owl exec` defaults to read-only unless you raise the sandbox level. Commands get no network access unless your sandbox settings allow it. Approval prompts (per command, per patch) follow your `approval_policy` setting. All of it is configurable - see [docs/config.md](config.md).
