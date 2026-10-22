//! party: clubs and parties for a household and the people around it,
//! served from an ESP32-S3. See `../spec/` for the design.
// Hardware FFI belongs in the firmware binary, never in application logic.
#![forbid(unsafe_code)]

pub mod api;
mod api_more;
pub mod bot;
#[cfg(test)]
mod channel_tests;
pub mod dsl;
#[cfg(test)]
mod event_tests;
pub mod events;
#[cfg(feature = "gate")]
pub mod gate;
pub mod groups;
pub mod ics;
pub mod identity;
pub mod invites;
pub mod mail;
pub mod messages;
pub mod notify;
pub mod outbound;
pub mod pin;
pub mod publish;
pub mod records;
pub mod rsvps;
pub mod series;
pub mod state;
pub mod table;
pub mod tz;
pub mod work;

pub use api::{App, Shared};
pub use state::Limits;
