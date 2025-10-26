//! Direct TCP interactions with the compiled fingermouse binary.

use anyhow::{Result, ensure};
use rstest::rstest;

use super::common::{HOST_MISMATCH, USER_MISSING, query_server, setup_server};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_direct_query_includes_basic_profile_details() -> Result<()> {
    let ctx = setup_server().await?;

    let response = query_server(ctx.address(), "alice").await?;
    ensure!(
        response.contains("User: alice"),
        "missing username in response: {response:?}"
    );
    ensure!(
        response.contains("Full name: Alice Example"),
        "missing full name in response: {response:?}"
    );
    ensure!(
        response.contains("Email: alice@test.host"),
        "missing email field in response: {response:?}"
    );
    ensure!(
        !response.contains("Plan:"),
        "non-verbose query should not include plan: {response:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_verbose_query_returns_plan_content() -> Result<()> {
    let ctx = setup_server().await?;

    let response = query_server(ctx.address(), "/W alice@test.host").await?;
    ensure!(
        response.contains("User: alice"),
        "missing username in verbose response: {response:?}"
    );
    ensure!(
        response.contains("Plan:"),
        "verbose response missing plan header: {response:?}"
    );
    ensure!(
        response.contains("Alice's Plan"),
        "verbose response missing plan contents: {response:?}"
    );
    ensure!(
        response.contains("Line 2"),
        "verbose response missing trailing plan line: {response:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_verbose_query_reports_missing_plan() -> Result<()> {
    let ctx = setup_server().await?;

    let response = query_server(ctx.address(), "/W bob").await?;
    ensure!(
        response.contains("User: bob"),
        "missing username for bob response: {response:?}"
    );
    ensure!(
        response.contains("(no plan)"),
        "expected missing plan marker: {response:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_query_handles_unknown_user_and_host_mismatch() -> Result<()> {
    let ctx = setup_server().await?;

    let missing_response = query_server(ctx.address(), "charlie").await?;
    ensure!(
        missing_response.contains(USER_MISSING),
        "missing user sentinel in response: {missing_response:?}"
    );

    let host_response = query_server(ctx.address(), "alice@wrong.host").await?;
    ensure!(
        host_response.contains(HOST_MISMATCH),
        "missing host mismatch sentinel in response: {host_response:?}"
    );

    Ok(())
}

#[rstest]
#[case("/W ALICE@test.host", "uppercase input")]
#[case("/W Alice@test.host", "mixed-case input")]
#[case("   /W alice@test.host", "leading whitespace")]
#[case("/W alice@test.host   ", "trailing whitespace")]
#[case("/W    alice@test.host", "collapsed whitespace")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_query_tolerates_case_and_whitespace_variations(
    #[case] query: &str,
    #[case] label: &str,
) -> Result<()> {
    let ctx = setup_server().await?;

    let response = query_server(ctx.address(), query).await?;
    ensure!(
        response.contains("User: alice"),
        "{label} should resolve to alice: {response:?}"
    );

    Ok(())
}
