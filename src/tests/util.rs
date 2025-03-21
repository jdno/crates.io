//! This module provides utility types and traits for managing a test session
//!
//! Tests start by using one of the `TestApp` constructors: `init`, `with_proxy`, or `full`.  This returns a
//! `TestAppBuilder` which provides convenience methods for creating up to one user, optionally with
//! a token.  The builder methods all return at least an initialized `TestApp` and a
//! `MockAnonymousUser`.  The `MockAnonymousUser` can be used to issue requests in an
//! unauthenticated session.
//!
//! A `TestApp` value provides raw access to the database through the `db` function and can
//! construct new users via the `db_new_user` function.  This function returns a
//! `MockCookieUser`, which can be used to generate one or more tokens via its `db_new_token`
//! function, which in turn returns a `MockTokenUser`.
//!
//! All three user types implement the `RequestHelper` trait which provides convenience methods for
//! constructing requests.  Some of these methods, such as `publish` are expected to fail for an
//! unauthenticated user (or for other reasons) and return a `Response<T>`.  The `Response<T>`
//! provides several functions to check the response status and deserialize the JSON response.
//!
//! `MockCookieUser` and `MockTokenUser` provide an `as_model` function which returns a reference
//! to the underlying database model value (`User` and `ApiToken` respectively).

use crate::{
    CategoryListResponse, CategoryResponse, CrateList, CrateResponse, GoodCrate, OkBool,
    OwnersResponse, VersionResponse,
};
use crates_io::middleware::session;
use crates_io::models::{ApiToken, CreatedApiToken, User};

use http::{Method, Request};

use axum::body::Bytes;
use axum::extract::connect_info::MockConnectInfo;
use chrono::NaiveDateTime;
use cookie::Cookie;
use crates_io::models::token::{CrateScope, EndpointScope};
use crates_io::util::token::PlainToken;
use http::header;
use secrecy::ExposeSecret;
use std::collections::HashMap;
use std::net::SocketAddr;
use tower_service::Service;

mod chaosproxy;
mod github;
pub mod insta;
mod mock_request;
mod response;
mod test_app;

pub(crate) use chaosproxy::ChaosProxy;
use mock_request::MockRequest;
pub use mock_request::MockRequestExt;
pub use response::Response;
pub use test_app::TestApp;

/// This function can be used to create a `Cookie` header for mock requests that
/// include cookie-based authentication.
///
/// ```
/// let cookie = encode_session_header(session_key, user_id);
/// request.header(header::COOKIE, &cookie);
/// ```
///
/// The implementation matches roughly what is happening inside of our
/// session middleware.
pub fn encode_session_header(session_key: &cookie::Key, user_id: i32) -> String {
    let cookie_name = "cargo_session";

    // build session data map
    let mut map = HashMap::new();
    map.insert("user_id".into(), user_id.to_string());

    // encode the map into a cookie value string
    let encoded = session::encode(&map);

    // put the cookie into a signed cookie jar
    let cookie = Cookie::build(cookie_name, encoded).finish();
    let mut jar = cookie::CookieJar::new();
    jar.signed_mut(session_key).add(cookie);

    // read the raw cookie from the cookie jar
    jar.get(cookie_name).unwrap().to_string()
}

/// A collection of helper methods for the 3 authentication types
///
/// Helper methods go through public APIs, and should not modify the database directly
pub trait RequestHelper {
    fn request_builder(&self, method: Method, path: &str) -> MockRequest;
    fn app(&self) -> &TestApp;

    /// Run a request that is expected to succeed
    #[track_caller]
    fn run<T>(&self, request: MockRequest) -> Response<T> {
        let router = self.app().router().clone();

        // Add a mock `SocketAddr` to the requests so that the `ConnectInfo`
        // extractor has something to extract.
        let mocket_addr = SocketAddr::from(([127, 0, 0, 1], 52381));
        let mut router = router.layer(MockConnectInfo(mocket_addr));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let axum_response = rt
            .block_on(router.call(request.map(hyper::Body::from)))
            .unwrap();

        // axum responses can't be converted directly to reqwest responses,
        // so we have to convert it to a hyper response first.
        let (parts, body) = axum_response.into_parts();
        let bytes = rt.block_on(hyper::body::to_bytes(body)).unwrap();
        let hyper_response = hyper::Response::from_parts(parts, bytes);

        Response::new(hyper_response.into())
    }

    /// Create a get request
    fn get_request(&self, path: &str) -> MockRequest {
        self.request_builder(Method::GET, path)
    }

    /// Create a POST request
    fn post_request(&self, path: &str) -> MockRequest {
        self.request_builder(Method::POST, path)
    }

    /// Issue a GET request
    #[track_caller]
    fn get<T>(&self, path: &str) -> Response<T> {
        self.run(self.get_request(path))
    }

    /// Issue a GET request that includes query parameters
    #[track_caller]
    fn get_with_query<T>(&self, path: &str, query: &str) -> Response<T> {
        let path_and_query = format!("{path}?{query}");
        let request = self.request_builder(Method::GET, &path_and_query);
        self.run(request)
    }

    /// Issue a PUT request
    #[track_caller]
    fn put<T>(&self, path: &str, body: impl Into<Bytes>) -> Response<T> {
        let mut request = self.request_builder(Method::PUT, path);
        *request.body_mut() = body.into();
        self.run(request)
    }

    /// Issue a DELETE request
    #[track_caller]
    fn delete<T>(&self, path: &str) -> Response<T> {
        let request = self.request_builder(Method::DELETE, path);
        self.run(request)
    }

    /// Issue a DELETE request with a body... yes we do it, for crate owner removal
    #[track_caller]
    fn delete_with_body<T>(&self, path: &str, body: impl Into<Bytes>) -> Response<T> {
        let mut request = self.request_builder(Method::DELETE, path);
        *request.body_mut() = body.into();
        self.run(request)
    }

    /// Search for crates matching a query string
    fn search(&self, query: &str) -> CrateList {
        self.get_with_query("/api/v1/crates", query).good()
    }

    /// Search for crates owned by the specified user.
    fn search_by_user_id(&self, id: i32) -> CrateList {
        self.search(&format!("user_id={id}"))
    }

    /// Publish the crate and run background jobs to completion
    ///
    /// Background jobs will publish to the git index and sync to the HTTP index.
    #[track_caller]
    fn publish_crate(&self, body: impl Into<Bytes>) -> Response<GoodCrate> {
        let response = self.put("/api/v1/crates/new", body);
        self.app().run_pending_background_jobs();
        response
    }

    /// Request the JSON used for a crate's page
    fn show_crate(&self, krate_name: &str) -> CrateResponse {
        let url = format!("/api/v1/crates/{krate_name}");
        self.get(&url).good()
    }

    /// Request the JSON used for a crate's minimal page
    fn show_crate_minimal(&self, krate_name: &str) -> CrateResponse {
        let url = format!("/api/v1/crates/{krate_name}");
        self.get_with_query(&url, "include=").good()
    }

    /// Request the JSON used to list a crate's owners
    fn show_crate_owners(&self, krate_name: &str) -> OwnersResponse {
        let url = format!("/api/v1/crates/{krate_name}/owners");
        self.get(&url).good()
    }

    /// Request the JSON used for a crate version's page
    fn show_version(&self, krate_name: &str, version: &str) -> VersionResponse {
        let url = format!("/api/v1/crates/{krate_name}/{version}");
        self.get(&url).good()
    }

    fn show_category(&self, category_name: &str) -> CategoryResponse {
        let url = format!("/api/v1/categories/{category_name}");
        self.get(&url).good()
    }

    fn show_category_list(&self) -> CategoryListResponse {
        let url = "/api/v1/categories";
        self.get(url).good()
    }
}

fn req(method: Method, path: &str) -> MockRequest {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::USER_AGENT, "conduit-test")
        .body(Bytes::new())
        .unwrap()
}

/// A type that can generate unauthenticated requests
pub struct MockAnonymousUser {
    app: TestApp,
}

impl RequestHelper for MockAnonymousUser {
    fn request_builder(&self, method: Method, path: &str) -> MockRequest {
        req(method, path)
    }

    fn app(&self) -> &TestApp {
        &self.app
    }
}

/// A type that can generate cookie authenticated requests
pub struct MockCookieUser {
    app: TestApp,
    user: User,
}

impl RequestHelper for MockCookieUser {
    fn request_builder(&self, method: Method, path: &str) -> MockRequest {
        let session_key = &self.app.as_inner().session_key();
        let cookie = encode_session_header(session_key, self.user.id);

        let mut request = req(method, path);
        request.header(header::COOKIE, &cookie);
        request
    }

    fn app(&self) -> &TestApp {
        &self.app
    }
}

impl MockCookieUser {
    /// Creates an instance from a database `User` instance
    pub fn new(app: &TestApp, user: User) -> Self {
        Self {
            app: app.clone(),
            user,
        }
    }

    /// Returns a reference to the database `User` model
    pub fn as_model(&self) -> &User {
        &self.user
    }

    /// Creates a token and wraps it in a helper struct
    ///
    /// This method updates the database directly
    pub fn db_new_token(&self, name: &str) -> MockTokenUser {
        self.db_new_scoped_token(name, None, None, None)
    }

    /// Creates a scoped token and wraps it in a helper struct
    ///
    /// This method updates the database directly
    pub fn db_new_scoped_token(
        &self,
        name: &str,
        crate_scopes: Option<Vec<CrateScope>>,
        endpoint_scopes: Option<Vec<EndpointScope>>,
        expired_at: Option<NaiveDateTime>,
    ) -> MockTokenUser {
        let token = self.app.db(|conn| {
            ApiToken::insert_with_scopes(
                conn,
                self.user.id,
                name,
                crate_scopes,
                endpoint_scopes,
                expired_at,
            )
            .unwrap()
        });
        MockTokenUser {
            app: self.app.clone(),
            token,
        }
    }
}

/// A type that can generate token authenticated requests
pub struct MockTokenUser {
    app: TestApp,
    token: CreatedApiToken,
}

impl RequestHelper for MockTokenUser {
    fn request_builder(&self, method: Method, path: &str) -> MockRequest {
        let mut request = req(method, path);
        request.header(header::AUTHORIZATION, self.token.plaintext.expose_secret());
        request
    }

    fn app(&self) -> &TestApp {
        &self.app
    }
}

impl MockTokenUser {
    /// Returns a reference to the database `ApiToken` model
    pub fn as_model(&self) -> &ApiToken {
        &self.token.model
    }

    pub fn plaintext(&self) -> &PlainToken {
        &self.token.plaintext
    }

    /// Add to the specified crate the specified owners.
    pub fn add_named_owners(&self, krate_name: &str, owners: &[&str]) -> Response<OkBool> {
        let url = format!("/api/v1/crates/{krate_name}/owners");
        let body = json!({ "owners": owners }).to_string();
        self.put(&url, body)
    }

    /// Add a single owner to the specified crate.
    pub fn add_named_owner(&self, krate_name: &str, owner: &str) -> Response<OkBool> {
        self.add_named_owners(krate_name, &[owner])
    }

    /// Remove from the specified crate the specified owners.
    pub fn remove_named_owners(&self, krate_name: &str, owners: &[&str]) -> Response<OkBool> {
        let url = format!("/api/v1/crates/{krate_name}/owners");
        let body = json!({ "owners": owners }).to_string();
        self.delete_with_body(&url, body)
    }

    /// Remove a single owner to the specified crate.
    pub fn remove_named_owner(&self, krate_name: &str, owner: &str) -> Response<OkBool> {
        self.remove_named_owners(krate_name, &[owner])
    }

    /// Add a user as an owner for a crate.
    pub fn add_user_owner(&self, krate_name: &str, username: &str) {
        self.add_named_owner(krate_name, username).good();
    }
}
