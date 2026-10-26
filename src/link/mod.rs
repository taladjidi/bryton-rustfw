//! The links to the outside, as bytes: the USB dev link protocol, USB mass storage, the phone link's formats, and the
//! profiler the dev link reads.
//! Re-exported at the crate root by their own names (`crate::dev`).

pub mod dev;
pub mod msc;
pub mod phone;
pub mod prof;
