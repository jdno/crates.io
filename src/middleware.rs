pub mod app;
mod balance_capacity;
mod block_traffic;
mod common_headers;
mod debug;
mod ember_html;
pub mod log_request;
pub mod normalize_path;
pub mod real_ip;
mod require_user_agent;
pub mod session;
mod static_or_continue;
mod update_metrics;

use ::sentry::integrations::tower as sentry_tower;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::Router;
use axum_extra::either::Either;
use axum_extra::middleware::option_layer;
use hyper::Body;
use std::time::Duration;
use tower::layer::util::Identity;
use tower_http::add_extension::AddExtensionLayer;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::{CompressionLayer, CompressionLevel};
use tower_http::timeout::{RequestBodyTimeoutLayer, TimeoutBody, TimeoutLayer};

use crate::app::AppState;
use crate::Env;

pub fn apply_axum_middleware(state: AppState, router: Router<(), TimeoutBody<Body>>) -> Router {
    let config = &state.config;
    let env = config.env();

    let capacity = config.db.primary.pool_size;
    if capacity >= 10 {
        info!(?capacity, "Enabling BalanceCapacity middleware");
    } else {
        info!("BalanceCapacity middleware not enabled. DB_PRIMARY_POOL_SIZE is too low.");
    }

    let middleware = tower::ServiceBuilder::new()
        .layer(CompressionLayer::new().quality(CompressionLevel::Fastest))
        .layer(RequestBodyTimeoutLayer::new(Duration::from_secs(30)))
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(sentry_tower::NewSentryLayer::new_from_top())
        .layer(sentry_tower::SentryHttpLayer::with_transaction())
        .layer(from_fn(self::real_ip::middleware))
        .layer(from_fn(log_request::log_requests))
        .layer(CatchPanicLayer::new())
        .layer(from_fn_with_state(
            state.clone(),
            update_metrics::update_metrics,
        ))
        // Optionally print debug information for each request
        // To enable, set the environment variable: `RUST_LOG=crates_io::middleware=debug`
        .layer(conditional_layer(env == Env::Development, || {
            from_fn(debug::debug_requests)
        }))
        .layer(from_fn_with_state(state.clone(), session::attach_session))
        .layer(from_fn_with_state(
            state.clone(),
            require_user_agent::require_user_agent,
        ))
        .layer(from_fn_with_state(
            state.clone(),
            block_traffic::block_by_ip,
        ))
        .layer(from_fn_with_state(
            state.clone(),
            block_traffic::block_by_header,
        ))
        .layer(from_fn_with_state(
            state.clone(),
            block_traffic::block_routes,
        ))
        .layer(from_fn_with_state(
            state.clone(),
            common_headers::add_common_headers,
        ))
        .layer(conditional_layer(env == Env::Development, || {
            from_fn(static_or_continue::serve_local_uploads)
        }))
        .layer(conditional_layer(config.serve_dist, || {
            from_fn(static_or_continue::serve_dist)
        }))
        .layer(conditional_layer(config.serve_html, || {
            from_fn_with_state(state.clone(), ember_html::serve_html)
        }))
        .layer(AddExtensionLayer::new(state.clone()))
        // This is currently the final middleware to run. If a middleware layer requires a database
        // connection, it should be run after this middleware so that the potential pool usage can be
        // tracked here.
        //
        // In production we currently have 2 equally sized pools (primary and a read-only replica).
        // Because such a large portion of production traffic is for download requests (which update
        // download counts), we consider only the primary pool here.
        .layer(conditional_layer(capacity >= 10, || {
            from_fn_with_state(state, balance_capacity::balance_capacity)
        }));

    router.layer(middleware)
}

pub fn conditional_layer<L, F: FnOnce() -> L>(condition: bool, layer: F) -> Either<L, Identity> {
    option_layer(condition.then(layer))
}
