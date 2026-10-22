//! Game Matcher: a household game-night planner served from an ESP32-C6.
//! See `../spec/` for the design and `../README.md` to run it.
// Hardware FFI belongs in the firmware binary, never in application logic.
#![forbid(unsafe_code)]

pub mod announce;
pub mod api;
pub mod catalog;
pub mod comments;
#[cfg(feature = "gate")]
pub mod gate;
pub mod ics;
pub mod identity;
pub mod mail;
pub mod messages;
pub mod notify;
pub mod outbound;
pub mod pin;
pub mod plans;
pub mod records;
pub mod recurring;
pub mod replies;
pub mod screen;
pub mod state;
pub mod store;
pub mod table;
pub mod views;

pub use api::App;
pub use state::Limits;
