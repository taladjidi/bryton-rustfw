//! The chips and radios, each one's protocol against the [`crate::Board`] primitives (BOARD.md has the hardware
//! facts): the panel, the touch controller, the sensor bus, the battery, the GNSS receiver, the Nordic BLE / ANT
//! chip and the ANT+ sensors it hears, the eMMC.

pub mod ambient;
pub mod battery;
pub mod emmc;
pub mod gnss;
pub mod nrf;
pub mod panel;
pub mod sensors;
pub mod touch;
