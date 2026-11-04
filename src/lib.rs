//! crashtrace: turn the Windows event log into a crash timeline.
//!
//! Read-only by design: it only queries or reads event data, never writes,
//! clears or changes anything on the machine.

pub mod cli;
pub mod event;
pub mod markers;
pub mod render;
pub mod report;
pub mod source;
pub mod stats;
pub mod timeline;
pub mod tz;
pub mod xml;

#[cfg(test)]
pub(crate) mod testutil;
