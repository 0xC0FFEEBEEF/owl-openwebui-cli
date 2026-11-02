#![allow(clippy::expect_used)]
use crate::test_codex::TEST_MODEL;
use std::path::Path;
use tempfile::TempDir;
use wiremock::MockServer;

pub struct TestCodexExecBuilder {
    home: TempDir,
    cwd: TempDir,
}

impl TestCodexExecBuilder {
    /// Write an owl config into the test settings home so the child is fully
    /// isolated from the machine it runs on: a `test` provider on the responses
    /// wire pointed at `base_url`, a pinned model name, and the server model
    /// listing switched off. Without this the child would fall back to the
    /// built-in Open WebUI URL (`http://localhost:8080/api/v1`) and could talk
    /// to whatever happens to be listening there, and a failed model listing
    /// would make `owl exec` stop with an error before the test's fixture or
    /// mock ever gets a chance to answer.
    fn write_test_config(&self, base_url: &str) {
        let contents = format!(
            "model = \"{TEST_MODEL}\"\n\
             model_provider = \"test\"\n\
             \n\
             [features]\n\
             remote_models = false\n\
             \n\
             [model_providers.test]\n\
             name = \"Test server\"\n\
             base_url = \"{base_url}\"\n\
             wire_api = \"responses\"\n"
        );
        std::fs::write(self.home.path().join("config.toml"), contents)
            .expect("write test config.toml");
    }

    fn cmd_base(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::new(
            codex_utils_cargo_bin::cargo_bin("codex-exec")
                .expect("should find binary for codex-exec"),
        );
        cmd.current_dir(self.cwd.path())
            .env("CODEX_HOME", self.home.path());
        cmd
    }

    /// For fixture-driven runs (`CODEX_RS_SSE_FIXTURE`): the base URL is never
    /// contacted because the model is pinned and the turn streams from the
    /// fixture file.
    pub fn cmd(&self) -> assert_cmd::Command {
        self.write_test_config("http://unused.local/v1");
        self.cmd_base()
    }

    pub fn cmd_with_server(&self, server: &MockServer) -> assert_cmd::Command {
        self.write_test_config(&format!("{}/v1", server.uri()));
        self.cmd_base()
    }

    pub fn cwd_path(&self) -> &Path {
        self.cwd.path()
    }

    pub fn home_path(&self) -> &Path {
        self.home.path()
    }
}

pub fn test_codex_exec() -> TestCodexExecBuilder {
    TestCodexExecBuilder {
        home: TempDir::new().expect("create temp home"),
        cwd: TempDir::new().expect("create temp cwd"),
    }
}
