use crate::{
    builders::CrateBuilder,
    util::{MockAnonymousUser, RequestHelper, TestApp},
};
use http::StatusCode;
use std::time::Duration;

const DB_HEALTHY_TIMEOUT: Duration = Duration::from_millis(2000);

#[test]
fn download_crate_with_broken_networking_primary_database() {
    let (app, anon, _, owner) = TestApp::init().with_chaos_proxy().with_token();
    app.db(|conn| {
        CrateBuilder::new("crate_name", owner.as_model().user_id)
            .version("1.0.0")
            .expect_build(conn)
    });

    // When the database connection is healthy downloads are redirected with the proper
    // capitalization, and missing crates or versions return a 404.

    assert_checked_redirects(&anon);

    // After networking breaks, preventing new database connections, the download endpoint should
    // do an unconditional redirect to the CDN, without checking whether the crate exists or what
    // the exact capitalization of crate name is.

    app.primary_db_chaosproxy().break_networking();
    assert_unconditional_redirects(&anon);

    // After restoring the network and waiting for the database pool to get healthy again redirects
    // should be checked again.

    app.primary_db_chaosproxy().restore_networking();
    app.as_inner()
        .primary_database
        .wait_until_healthy(DB_HEALTHY_TIMEOUT)
        .expect("the database did not return healthy");

    assert_checked_redirects(&anon);
}

fn assert_checked_redirects(anon: &MockAnonymousUser) {
    anon.get::<()>("/api/v1/crates/crate_name/1.0.0/download")
        .assert_redirect_ends_with("/crate_name/crate_name-1.0.0.crate");

    anon.get::<()>("/api/v1/crates/Crate-Name/1.0.0/download")
        .assert_redirect_ends_with("/crate_name/crate_name-1.0.0.crate");

    anon.get::<()>("/api/v1/crates/crate_name/2.0.0/download")
        .assert_not_found();

    anon.get::<()>("/api/v1/crates/awesome-project/1.0.0/download")
        .assert_not_found();
}

fn assert_unconditional_redirects(anon: &MockAnonymousUser) {
    anon.get::<()>("/api/v1/crates/crate_name/1.0.0/download")
        .assert_redirect_ends_with("/crate_name/crate_name-1.0.0.crate");

    anon.get::<()>("/api/v1/crates/Crate-Name/1.0.0/download")
        .assert_redirect_ends_with("/Crate-Name/Crate-Name-1.0.0.crate");

    anon.get::<()>("/api/v1/crates/crate_name/2.0.0/download")
        .assert_redirect_ends_with("/crate_name/crate_name-2.0.0.crate");

    anon.get::<()>("/api/v1/crates/awesome-project/1.0.0/download")
        .assert_redirect_ends_with("/awesome-project/awesome-project-1.0.0.crate");
}

#[test]
fn http_error_with_unhealthy_database() {
    let (app, anon) = TestApp::init().with_chaos_proxy().empty();

    let response = anon.get::<()>("/api/v1/summary");
    assert_eq!(response.status(), StatusCode::OK);

    app.primary_db_chaosproxy().break_networking();

    let response = anon.get::<()>("/api/v1/summary");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    app.primary_db_chaosproxy().restore_networking();
    app.as_inner()
        .primary_database
        .wait_until_healthy(DB_HEALTHY_TIMEOUT)
        .expect("the database did not return healthy");

    let response = anon.get::<()>("/api/v1/summary");
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn fallback_to_replica_returns_user_info() {
    const URL: &str = "/api/v1/users/foo";

    let (app, _, owner) = TestApp::init()
        .with_replica()
        .with_chaos_proxy()
        .with_user();
    app.db_new_user("foo");
    app.primary_db_chaosproxy().break_networking();

    // When the primary database is down, requests are forwarded to the replica database
    let response = owner.get::<()>(URL);
    assert_eq!(response.status(), 200);

    // restore primary database connection
    app.primary_db_chaosproxy().restore_networking();
    app.as_inner()
        .primary_database
        .wait_until_healthy(DB_HEALTHY_TIMEOUT)
        .expect("the database did not return healthy");
}

#[test]
fn restored_replica_returns_user_info() {
    const URL: &str = "/api/v1/users/foo";

    let (app, _, owner) = TestApp::init()
        .with_replica()
        .with_chaos_proxy()
        .with_user();
    app.db_new_user("foo");
    app.primary_db_chaosproxy().break_networking();
    app.replica_db_chaosproxy().break_networking();

    // When both primary and replica database are down, the request returns an error
    let response = owner.get::<()>(URL);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Once the replica database is restored, it should serve as a fallback again
    app.replica_db_chaosproxy().restore_networking();
    app.as_inner()
        .read_only_replica_database
        .as_ref()
        .expect("no replica database configured")
        .wait_until_healthy(DB_HEALTHY_TIMEOUT)
        .expect("the database did not return healthy");

    let response = owner.get::<()>(URL);
    assert_eq!(response.status(), StatusCode::OK);

    // restore connection
    app.primary_db_chaosproxy().restore_networking();
    app.as_inner()
        .primary_database
        .wait_until_healthy(DB_HEALTHY_TIMEOUT)
        .expect("the database did not return healthy");
}

#[test]
fn restored_primary_returns_user_info() {
    const URL: &str = "/api/v1/users/foo";

    let (app, _, owner) = TestApp::init()
        .with_replica()
        .with_chaos_proxy()
        .with_user();
    app.db_new_user("foo");
    app.primary_db_chaosproxy().break_networking();
    app.replica_db_chaosproxy().break_networking();

    // When both primary and replica database are down, the request returns an error
    let response = owner.get::<()>(URL);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Once the replica database is restored, it should serve as a fallback again
    app.primary_db_chaosproxy().restore_networking();
    app.as_inner()
        .primary_database
        .wait_until_healthy(DB_HEALTHY_TIMEOUT)
        .expect("the database did not return healthy");

    let response = owner.get::<()>(URL);
    assert_eq!(response.status(), StatusCode::OK);
}
