//! Where the rider is and where the route goes: the course model and its cues, the climbs ahead, the barometric
//! altitude. Pure logic. Re-exported at the crate root by their own names (`crate::course`).

pub mod altitude;
pub mod climbs_ahead;
pub mod course;
pub mod segment;
pub mod segment_match;
pub mod sun;
