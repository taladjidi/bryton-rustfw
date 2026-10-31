//! Files: the FAT32 volume on the eMMC (read, and written by the ride recorder only), the FIT ride file (written
//! and read), the recorder, the IGN map packs (read), and what the stock firmware left on the volume (its rides,
//! settings, planned workouts and courses).

pub mod bikes;
pub mod bytes;
pub mod copies;
pub mod course_file;
pub mod fat;
pub mod faultlog;
pub mod fit;
pub mod fitread;
pub mod history;
pub mod ign;
pub mod inflate;
pub mod ini;
pub mod named;
pub mod osm;
pub mod recorder;
pub mod route;
pub mod screenshot;
pub mod stock;
pub mod thresholds;
pub mod trash;
pub mod volume;
pub mod workout_file;

/// The `N` bytes of `b` from `i`, zeros when `b` is too short (a malformed record reads as empty, never panics).
pub(crate) fn take<const N: usize>(b: &[u8], i: usize) -> [u8; N] {
    i.checked_add(N).and_then(|end| b.get(i..end)).and_then(|s| s.try_into().ok()).unwrap_or([0; N])
}
