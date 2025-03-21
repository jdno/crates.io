use crate::builders::{CrateBuilder, DependencyBuilder, PublishBuilder};
use crate::util::{RequestHelper, TestApp};
use http::StatusCode;
use insta::assert_json_snapshot;

#[test]
fn features_version_2() {
    let (app, _, user, token) = TestApp::full().with_token();

    app.db(|conn| {
        // Insert a crate directly into the database so that foo_new can depend on it
        CrateBuilder::new("bar", user.as_model().id).expect_build(conn);
    });

    let dependency = DependencyBuilder::new("bar");

    let crate_to_publish = PublishBuilder::new("foo", "1.0.0")
        .dependency(dependency)
        .feature("new_feat", &["dep:bar", "bar?/feat"])
        .feature("old_feat", &[]);
    token.publish_crate(crate_to_publish).good();

    let crates = app.crates_from_index_head("foo");
    assert_json_snapshot!(crates);
}

#[test]
fn invalid_feature_name() {
    let (app, _, _, token) = TestApp::full().with_token();

    let crate_to_publish = PublishBuilder::new("foo", "1.0.0").feature("~foo", &[]);
    let response = token.publish_crate(crate_to_publish);
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_snapshot!(response.into_json());
    assert!(app.stored_files().is_empty());
}

#[test]
fn invalid_feature() {
    let (app, _, _, token) = TestApp::full().with_token();

    let crate_to_publish = PublishBuilder::new("foo", "1.0.0").feature("foo", &["!bar"]);
    let response = token.publish_crate(crate_to_publish);
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_snapshot!(response.into_json());
    assert!(app.stored_files().is_empty());
}

#[test]
fn too_many_features() {
    let (app, _, _, token) = TestApp::full()
        .with_config(|config| {
            config.max_features = 3;
        })
        .with_token();

    let publish_builder = PublishBuilder::new("foo", "1.0.0")
        .feature("one", &[])
        .feature("two", &[])
        .feature("three", &[])
        .feature("four", &[])
        .feature("five", &[]);
    let response = token.publish_crate(publish_builder);
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_snapshot!(response.into_json());
    assert!(app.stored_files().is_empty());
}

#[test]
fn too_many_features_with_custom_limit() {
    let (app, _, user, token) = TestApp::full()
        .with_config(|config| {
            config.max_features = 3;
        })
        .with_token();

    app.db(|conn| {
        CrateBuilder::new("foo", user.as_model().id)
            .max_features(4)
            .expect_build(conn)
    });

    let publish_builder = PublishBuilder::new("foo", "1.0.0")
        .feature("one", &[])
        .feature("two", &[])
        .feature("three", &[])
        .feature("four", &[])
        .feature("five", &[]);
    let response = token.publish_crate(publish_builder);
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_snapshot!(response.into_json());
    assert!(app.stored_files().is_empty());

    let publish_builder = PublishBuilder::new("foo", "1.0.0")
        .feature("one", &[])
        .feature("two", &[])
        .feature("three", &[])
        .feature("four", &[]);
    token.publish_crate(publish_builder).good();
}

#[test]
fn too_many_enabled_features() {
    let (app, _, _, token) = TestApp::full()
        .with_config(|config| {
            config.max_features = 3;
        })
        .with_token();

    let publish_builder = PublishBuilder::new("foo", "1.0.0")
        .feature("default", &["one", "two", "three", "four", "five"]);
    let response = token.publish_crate(publish_builder);
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_snapshot!(response.into_json());
    assert!(app.stored_files().is_empty());
}

#[test]
fn too_many_enabled_features_with_custom_limit() {
    let (app, _, user, token) = TestApp::full()
        .with_config(|config| {
            config.max_features = 3;
        })
        .with_token();

    app.db(|conn| {
        CrateBuilder::new("foo", user.as_model().id)
            .max_features(4)
            .expect_build(conn)
    });

    let publish_builder = PublishBuilder::new("foo", "1.0.0")
        .feature("default", &["one", "two", "three", "four", "five"]);
    let response = token.publish_crate(publish_builder);
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_snapshot!(response.into_json());
    assert!(app.stored_files().is_empty());

    let publish_builder =
        PublishBuilder::new("foo", "1.0.0").feature("default", &["one", "two", "three", "four"]);
    token.publish_crate(publish_builder).good();
}
