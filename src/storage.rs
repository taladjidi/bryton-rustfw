//! Files: the FAT32 volume on the eMMC (read only so far), the FIT ride file (written and read), and what the stock
//! firmware left on the volume (its rides, settings and planned workouts).

pub mod fat;
pub mod fit;
pub mod fitread;
pub mod history;
pub mod ini;
pub mod stock;
pub mod workout;
