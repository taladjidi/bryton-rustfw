//! The binary's side of the board: the unsafe primitives (hw.rs, the only `unsafe` in the firmware, AGENTS.md) and
//! the USB link (usb.rs). `main.rs` implements [`bryton_rustfw::Board`] with them.

pub mod hw;
pub mod usb;
