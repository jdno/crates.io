use crate::builders::CrateBuilder;
use crate::{RequestHelper, TestApp};

use diesel::prelude::*;
use http::StatusCode;

#[test]
fn can_hit_read_only_endpoints_in_read_only_mode() {
    let (_app, anon) = TestApp::init()
        .without_test_database_pool()
        .with_config(|config| {
            config.db.primary.read_only_mode = true;
        })
        .empty();

    let response = anon.get::<()>("/api/v1/crates");
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn cannot_hit_endpoint_which_writes_db_in_read_only_mode() {
    let (app, _, user, token) = TestApp::init()
        .without_test_database_pool()
        .with_config(|config| {
            config.db.primary.read_only_mode = true;
        })
        .with_token();

    app.db(|conn| {
        CrateBuilder::new("foo_yank_read_only", user.as_model().id)
            .version("1.0.0")
            .expect_build(conn);
    });

    let response = token.delete::<()>("/api/v1/crates/foo_yank_read_only/1.0.0/yank");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn can_download_crate_in_read_only_mode() {
    let (app, anon, user) = TestApp::init()
        .without_test_database_pool()
        .with_config(|config| {
            config.db.primary.read_only_mode = true;
        })
        .with_user();

    app.db(|conn| {
        CrateBuilder::new("foo_download_read_only", user.as_model().id)
            .version("1.0.0")
            .expect_build(conn);
    });

    let response = anon.get::<()>("/api/v1/crates/foo_download_read_only/1.0.0/download");
    assert_eq!(response.status(), StatusCode::FOUND);

    // We're in read only mode so the download should not have been counted
    app.db(|conn| {
        use crates_io::schema::version_downloads;
        use diesel::dsl::sum;

        let dl_count: Result<Option<i64>, _> = version_downloads::table
            .select(sum(version_downloads::downloads))
            .get_result(conn);
        assert_ok_eq!(dl_count, None);
    })
}
