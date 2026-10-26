//! Training logic, all pure and fed a second at a time: power metrics and the power curve, pacing, the ramp test,
//! heart rate variability, planned workouts, reminders and the home trainer. Re-exported at the crate root by
//! their own names (`crate::power`).

pub mod alerts;
pub mod hrv;
pub mod hrvt;
pub mod pacing;
pub mod power;
pub mod power_curve;
pub mod ramp;
pub mod trainer;
pub mod workout;
