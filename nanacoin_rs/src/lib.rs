//! A bounded household ledger, independent of its HTTP and ESP32 adapters.
pub mod api;
pub mod auth;
mod client;
pub mod domain;
pub mod journal;
mod json;
pub mod offers;
