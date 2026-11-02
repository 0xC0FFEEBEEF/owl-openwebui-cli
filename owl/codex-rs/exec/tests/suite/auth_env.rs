#![allow(clippy::unwrap_used, clippy::expect_used)]
use codex_utils_cargo_bin::find_resource;
use core_test_support::responses::ev_completed;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex_exec::test_codex_exec;

/// owl never authenticates with the upstream `CODEX_API_KEY` variable: only the
/// user's provider keys (or owl's own saved auth) may add Authorization headers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_ignores_codex_api_key_env_var() -> anyhow::Result<()> {
    let test = test_codex_exec();
    let server = start_mock_server().await;
    let repo_root = find_resource!(".")?;

    let request_recorder = mount_sse_once(&server, sse(vec![ev_completed("request_0")])).await;

    test.cmd_with_server(&server)
        .env("CODEX_API_KEY", "dummy")
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(&repo_root)
        .arg("echo testing codex api key")
        .assert()
        .success();

    let request = request_recorder.single_request();
    assert_eq!(
        request.header("Authorization"),
        None,
        "owl must not authenticate with CODEX_API_KEY against the user's server"
    );

    Ok(())
}
