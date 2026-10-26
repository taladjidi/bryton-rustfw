//! The app's parts, by concern: [`App`](crate::App) itself (its state, `start` and `step`) stays in lib.rs, and each
//! file here adds an `impl App` block for one concern. They are private modules re-exported whole from the crate
//! root, so paths stay as they were (`bryton_rustfw::POLL_MS`, `crate::Settings`).

mod ahead;
mod barometer;
mod clock;
mod courses;
mod device;
mod drive;
mod editor;
mod fields;
mod fitness;
mod ftms;
mod gps;
mod graphs;
mod input;
mod link;
mod menus;
mod more;
mod notifs;
mod pace;
mod phone;
mod profile;
mod radio;
mod riding;
mod screens;
mod settings;
mod stock_data;
mod trainer;
mod workouts;

pub use barometer::*;
pub use clock::*;
pub use device::*;
pub(crate) use editor::*;
pub(crate) use fields::*;
pub use ftms::*;
pub use gps::*;
pub(crate) use graphs::*;
pub use input::*;
pub use link::*;
pub use menus::*;
pub(crate) use pace::PaceState;
pub use phone::*;
pub use radio::*;
pub use riding::*;
pub use screens::*;
pub use settings::*;
pub use stock_data::*;
pub(crate) use trainer::SensorStore;
pub(crate) use workouts::GaugeShown;
