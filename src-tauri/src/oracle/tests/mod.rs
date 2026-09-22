//! Oracle tests, split by the area they exercise. Shared harness pieces —
//! the environment lock, temp runtimes, fixtures — live in [`support`].

mod commands;
mod endpoint;
mod endpoint_query;
mod endpoint_query_unit;
mod folder;
mod host;
mod indexing;
mod model_download;
mod real_model_e2e;
mod status_query;
mod support;
mod warm_slot;
