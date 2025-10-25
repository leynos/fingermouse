//! Direct TCP interactions with the compiled fingermouse binary.

use anyhow::{Result, ensure};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_query_tolerates_case_and_whitespace_variations() -> Result<()> {
    let ctx = setup_server().await?;

    let uppercase = query_server(ctx.address(), "/W ALICE@test.host").await?;
    ensure!(
        uppercase.contains("User: alice"),
        "uppercase query should match alice: {uppercase:?}"
    );

    let mixed_case = query_server(ctx.address(), "/W Alice@test.host").await?;
    ensure!(
        mixed_case.contains("User: alice"),
        "mixed-case query should match alice: {mixed_case:?}"
    );

    let leading_whitespace = query_server(ctx.address(), "   /W alice@test.host").await?;
    ensure!(
        leading_whitespace.contains("User: alice"),
        "leading whitespace should be ignored: {leading_whitespace:?}"
    );

    let trailing_whitespace = query_server(ctx.address(), "/W alice@test.host   ").await?;
    ensure!(
        trailing_whitespace.contains("User: alice"),
        "trailing whitespace should be ignored: {trailing_whitespace:?}"
    );

    let internal_whitespace = query_server(ctx.address(), "/W    alice@test.host").await?;
    ensure!(
        internal_whitespace.contains("User: alice"),
        "collapsed whitespace should be tolerated: {internal_whitespace:?}"
    );

    Ok(())
}
