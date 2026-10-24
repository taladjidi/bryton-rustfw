//! The app's parts, by concern: [`App`](crate::App) itself (its state, `start` and `step`) stays in lib.rs, and each
//! file here adds an `impl App` block for one concern. They are private modules re-exported whole from the crate
//! root, so paths stay as they were (`bryton_rustfw::POLL_MS`, `crate::Settings`).

mod clock;
mod courses;
mod device;
mod drive;
mod fields;
mod gps;
mod graphs;
mod input;
mod menus;
mod radio;
mod riding;
mod screens;
mod settings;
mod stock_data;
mod workouts;

pub use clock::*;
pub use device::*;
pub(crate) use fields::*;
pub use gps::*;
pub(crate) use graphs::*;
pub use input::*;
pub use menus::*;
pub use radio::*;
pub use riding::*;
pub use screens::*;
pub use settings::*;
pub use stock_data::*;
pub(crate) use workouts::GaugeShown;
