//! The chips and radios, each one's protocol against the [`crate::Board`] primitives (BOARD.md has the hardware
//! facts): the panel, the touch controller, the sensor bus, the battery, the GNSS receiver, the Nordic BLE / ANT
//! chip (ANT+ sensors, and BLE: the phone link) and the ANT+ sensors it hears, the eMMC.

pub mod ambient;
pub mod battery;
pub mod ble;
pub mod emmc;
pub mod fec;
pub mod fecble;
pub mod ftms;
pub mod gnss;
pub mod nrf;
pub mod panel;
pub mod powercal;
pub mod sensors;
pub mod touch;
