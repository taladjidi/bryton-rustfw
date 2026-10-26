//! Climb pacing: the power to ride each section of a climb to reach the top soonest with the W' the rider has. Pure
//! logic, floats (the Cortex-M7 has an FPU), no heap, every loop bounded; it formats nothing.
//!
//! **Model.** The rider's power is limited by critical power (CP) and W': above CP the work `(P - CP) t` is paid from
//! W' and must not pass the budget (Monod and Scherrer 1965; the fit is `power_curve::fit_cp`). W' recovery below CP
//! is ignored inside a climb (a climb is minutes long; the live W' balance at its start is the budget, and the plan
//! is made again at each section from the balance then, so what recovery or overspending happened is picked up).
//!
//! **Speed from power.** The steady-state power equation of Martin et al. 1998 ("Validation of a mathematical model
//! for road cycling power", J Appl Biomech 14(3):276-291): `P eta = v (m g (grade + Crr) + CdA rho v^2 / 2)`, wheel
//! power against gravity, rolling resistance and air drag; no wind, no kinetic energy (steady speed per section),
//! grade for sin(angle) and 1 for cos(angle) (3 % error at 20 %). Solved for `v` by Newton's method ([`speed`]).
//!
//! **Allocation.** Minimise the total time `sum L_i / v_i` for `sum (P_i - CP) L_i / v_i <= W'`, with the power of
//! each section a function of its speed, `P = v (k + a v^2) / eta`. The stationarity condition with the multiplier
//! `mu` is `2 a v^3 / eta + CP = 1 / mu`: it does not contain the grade, so **every section not at a bound rides at
//! the same speed** `v` (the "constant speed" mode of the optimal-control studies: more power where it is steep,
//! as Swain 1997 found, but the speed is one number). A section is at CP when `v` is under the speed it makes at CP,
//! at the highest power when over the speed it makes there; those two speeds are solved once per section ([`speed`],
//! Newton, 64 solves). The W' spent only rises with `v`, so the plan is one bisection on `v` (13 passes of 32
//! sections for half a percent of the budget), no solver inside it. On one constant grade it gives
//! `P = CP + W'/T`. The first version bisected `lambda` with a Newton solve for every section at every step: 9.7k
//! Newton steps for 32 sections, about 1.9 ms at 200 MHz; now about 210 steps and 13 passes, about 75 us (the
//! `replan_cost` test counts them), with the same or a slightly better time on the `compare_cases` cases.
//!
//! **Survey** (what was read, what was not; `inferred` marks a claim from a search summary only):
//!
//! | Work | Model | Method | Gain over even power |
//! |---|---|---|---|
//! | Swain 1997, MSSE 29(8) | power equation, fixed mean power | numerical search | savings from varying power on hills and wind |
//! | Atkinson, Peacock, Passfield 2007 (Eur J Appl Physiol, PubMed 17497402), Atkinson & Brunskill 2000 | Martin-type model, no W' | simulation | +-10 % power variation: 126 s (hilly) and 51 s (windy) on Swain's courses; 26 s on a 40 km flat TT |
//! | Ashtiani, Vahidi, arXiv 2007.11393 | fatigue / recovery models fitted per rider | Pontryagin: four modes (all-out, coast, CP, constant speed), dynamic programming | 3 % in a pilot with real-time pacing; 24 % in simulation against a self-paced ride |
//! | Gordon 2005, Cangley 2011, de Jong 2017, Wolf et al. 2019, Sundstrom, Boswell 2012 | W' / CP and variants | NLP, dynamic programming | not read here (`inferred`: all find more power on steeper parts) |
//! | Best Bike Split, Garmin PacePro / PowerGuide, Bryton | proprietary | not documented | not documented |
//!
//! Decisions for a 200 MHz core replanning once a section: the model and the "constant speed on the interior" closed
//! form above stay (it is the Pontryagin result for this budget, without recovery); dynamic programming and NLP
//! (state: W' balance, a grid of speeds) cost thousands of times more for a gain only when W' recovery on descents
//! inside a climb matters, which a 200 m grid on a climb rarely shows; a Cardano cube root is not needed (the cubic is
//! avoided, not solved faster); warm starts, merged sections and an early exit on steep grades would save under 20 us
//! of the 75 and were not done. W' recovery (Skiba) on a descent inside the climb, wind and CdA stay out: they break
//! the one-speed form and the settings do not hold them. The top power `CP + W'/60` (what the balance would spend in a
//! minute) is kept as the sprint limit; it binds only on very short steep sections.
//!
//! Constants are those of a typical road rider climbing on the hoods; a setting later if the owner asks.

/// Sections a plan holds at most (the course's climbs are cut into at most this many).
pub const MAX: usize = 32;

/// Drag area on a climb, hoods, m^2 (Martin 1998 measured 0.32 to 0.41 for road positions).
pub const CDA: f32 = 0.40;
/// Rolling resistance coefficient of road tyres on asphalt (typical 0.004 to 0.006).
pub const CRR: f32 = 0.005;
/// Drivetrain efficiency (Martin 1998: 97.7 % was their bike's).
pub const ETA: f32 = 0.977;
/// Air density, kg/m^3 (15 C at 150 m; thinner air at altitude is ignored, a conservative error on climbs).
pub const RHO: f32 = 1.2;
const G: f32 = 9.81;
/// The fastest speed a section is solved to, m/s (90 km/h): descents are not paced.
const VMAX: f32 = 25.0;
/// The slowest, m/s.
const VMIN: f32 = 0.3;
/// Largest grade used either way (a data error beyond it).
const GRADE_MAX: f32 = 0.4;

/// A piece of the climb.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sec {
    /// Its length, m.
    pub len_m: u16,
    /// Its grade, % x 10 (negative: down).
    pub grade_x10: i16,
}

/// The rider and the bike.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rider {
    /// Critical power, W.
    pub cp: f32,
    /// The whole W', J (it sets the highest power asked: CP + W' / 60 s).
    pub w_full: f32,
    /// The W' to spend on the climb, J (the balance now, at most `w_full`).
    pub budget: f32,
    /// Rider and bike, kg.
    pub mass: f32,
}

/// A plan: a power for each section and the time it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The power of each section, W (the first `n`).
    pub watts: [u16; MAX],
    /// Sections planned.
    pub n: u8,
    /// The predicted time over all of them, s.
    pub total_s: u32,
}

impl Plan {
    /// No plan.
    pub const EMPTY: Plan = Plan { watts: [0; MAX], n: 0, total_s: 0 };
}

/// The square root (Newton from 32, above every root asked; `core` has none).
fn sqrt(s: f32) -> f32 {
    let mut x = 32.0;
    for _ in 0..12 {
        x = (x + s / x) * 0.5;
    }
    x
}

/// The power equation's two terms for a section: `k` (gravity and rolling, N) and `a` (drag, N s^2/m^2).
fn coefs(grade_x10: i16, mass: f32) -> (f32, f32) {
    let g = (f32::from(grade_x10) / 1000.0).clamp(-GRADE_MAX, GRADE_MAX);
    (mass * G * (g + CRR), 0.5 * CDA * RHO)
}

/// Newton steps taken by [`speed`] (tests count them to bound a plan's cost).
#[cfg(test)]
extern crate std;
#[cfg(test)]
std::thread_local!(static STEPS: core::cell::Cell<u32> = const { core::cell::Cell::new(0) });
// Passes over the sections by the plan's search (tests count them too).
#[cfg(test)]
std::thread_local!(static PASSES: core::cell::Cell<u32> = const { core::cell::Cell::new(0) });

/// The steady speed at pedal power `p` W on a section with terms `k`, `a`, m/s: Newton's method on the wheel power
/// `f(v) = v (k + a v^2)`, which is increasing and convex for `v` above the speed where `k + a v^2` is zero (all
/// `v > 0` uphill). It starts above the root (`f(v) >= k v` gives `wheel / k` as an upper bound) and so descends to
/// it monotonically, in about 5 steps; a step that leaves the valid range is halved toward its floor, and the loop
/// is bounded.
pub fn speed(p: f32, k: f32, a: f32) -> f32 {
    let wheel = p.max(0.0) * ETA;
    let f = |v: f32| v * (k + a * v * v);
    let floor = if k < 0.0 { sqrt(-k / a).min(VMAX) } else { 0.0 };
    if f(VMAX) <= wheel {
        return VMAX;
    }
    let mut v = if k > 0.0 { (wheel / k).min(VMAX) } else { VMAX };
    for _ in 0..16 {
        #[cfg(test)]
        STEPS.with(|s| s.set(s.get() + 1));
        let next = v - (f(v) - wheel) / (k + 3.0 * a * v * v).max(1e-3);
        let next = if next > floor { next } else { (v + floor) * 0.5 }; // also when NaN
        let step = v - next;
        v = next;
        if step < 1e-3 && step > -1e-3 {
            break;
        }
    }
    v.clamp(VMIN, VMAX)
}

/// The fastest way up `secs` (at most [`MAX`]) for `r`: a power for each section and the time. An empty plan for no
/// sections or a CP that is not positive; a budget of nothing gives CP everywhere. Never NaN: inputs are clamped.
///
/// ```
/// use bryton_rustfw::pacing::{plan, Rider, Sec};
/// // 3 km at 6 %, CP 250 W, 20 kJ of W': about 12 minutes, 250 + 20000 / T watts all the way
/// let secs = [Sec { len_m: 500, grade_x10: 60 }; 6];
/// let r = Rider { cp: 250.0, w_full: 20_000.0, budget: 20_000.0, mass: 84.0 };
/// let p = plan(&secs, &r);
/// assert!((p.total_s as i32 - 700).abs() < 100);
/// assert_eq!(p.watts[0], p.watts[5]);
/// assert!((p.watts[0] as i32 - (250 + 20_000 / p.total_s as i32)).abs() <= 2);
/// ```
pub fn plan(secs: &[Sec], r: &Rider) -> Plan {
    let n = secs.len().min(MAX);
    let cp = if r.cp.is_finite() { r.cp } else { 0.0 };
    if n == 0 || cp < 1.0 {
        return Plan::EMPTY;
    }
    let mass = if r.mass.is_finite() { r.mass.max(1.0) } else { 1.0 };
    let w_full = if r.w_full.is_finite() { r.w_full.max(0.0) } else { 0.0 };
    let budget = if r.budget.is_finite() { r.budget.clamp(0.0, w_full) } else { 0.0 };
    let hi = cp + w_full / 60.0;
    // per section: length, k, speeds at CP and at the highest power (the clamps of the power band)
    let mut sec = [(0f32, 0f32, 0f32, 0f32); MAX];
    for (s, i) in sec.iter_mut().zip(secs) {
        let (k, a) = coefs(i.grade_x10, mass);
        *s = (f32::from(i.len_m), k, speed(cp, k, a), speed(hi, k, a));
    }
    let sec = sec.get(..n).unwrap_or(&[]);
    let a = 0.5 * CDA * RHO;
    // The W' spent and the time when every section not at a clamp rides at speed `v`, and the powers. Power from speed
    // is explicit (`P = v (k + a v^2) / eta`), so no section is solved: at or below its CP speed a section rides at
    // CP (no W'), at or above its top-power speed at the top power, else at `v`.
    let measure = |v: f32, p: &mut [f32; MAX]| {
        #[cfg(test)]
        PASSES.with(|s| s.set(s.get() + 1));
        let (mut work, mut total, inv) = (0f32, 0f32, 1.0 / v);
        for (&(len, k, v_cp, v_top), w) in sec.iter().zip(p.iter_mut()) {
            let (pw, t) = if v <= v_cp {
                (cp, len / v_cp)
            } else if v >= v_top {
                (hi, len / v_top)
            } else {
                ((v * (k + a * v * v) / ETA).clamp(cp, hi), len * inv)
            };
            *w = pw;
            (work, total) = (work + (pw - cp) * t, total + t);
        }
        (work, total)
    };
    let v_lo = sec.iter().fold(VMAX, |m, s| m.min(s.2)); // every section at CP
    let v_up = sec.iter().fold(VMIN, |m, s| m.max(s.3)); // every section at the top power
    let mut best = [cp; MAX]; // the powers kept: the most that spends no more than the budget
    let mut total = measure(v_lo, &mut best).1;
    if budget >= 1.0 {
        let mut p = [cp; MAX];
        let (work, t) = measure(v_up, &mut p);
        if work <= budget {
            (best, total) = (p, t); // the budget covers every section at the highest power
        } else {
            // bisect the speed: the W' spent only rises with it, from nothing at `v_lo`
            let (mut lo, mut up) = (v_lo, v_up);
            for _ in 0..24 {
                let v = (lo + up) * 0.5;
                let (work, t) = measure(v, &mut p);
                if work > budget {
                    up = v;
                } else {
                    (lo, best, total) = (v, p, t);
                    if work >= budget * 0.995 {
                        break; // within half a percent of the budget: enough
                    }
                }
            }
        }
    }
    let mut watts = [0u16; MAX];
    for (w, &p) in watts.iter_mut().zip(&best) {
        *w = (p + 0.5).clamp(0.0, 65_535.0) as u16;
    }
    Plan { watts, n: n as u8, total_s: (total + 0.5).clamp(0.0, 1e9) as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rider(budget: f32) -> Rider {
        Rider { cp: 250.0, w_full: 20_000.0, budget, mass: 84.0 }
    }

    #[test]
    fn constant_grade_spends_the_w_prime_at_a_constant_power() {
        let secs = [Sec { len_m: 400, grade_x10: 70 }; 8];
        let p = plan(&secs, &rider(15_000.0));
        let t = p.total_s as f32;
        assert!(p.watts[..8].iter().all(|&w| w == p.watts[0]));
        let spent = (f32::from(p.watts[0]) - 250.0) * t;
        assert!((spent - 15_000.0).abs() < 300.0, "{spent}");
        assert!((f32::from(p.watts[0]) - (250.0 + 15_000.0 / t)).abs() < 2.0);
    }

    #[test]
    fn steeper_gets_more_power_and_the_plan_beats_even_pacing() {
        let secs = [Sec { len_m: 500, grade_x10: 30 }, Sec { len_m: 500, grade_x10: 100 }];
        let r = rider(10_000.0);
        let p = plan(&secs, &r);
        assert!(p.watts[1] > p.watts[0], "{:?}", &p.watts[..2]);
        // even pacing at the same W' (the same power on both sections)
        let (k0, k1) = (coefs(30, 84.0), coefs(100, 84.0));
        let even = |w: f32| 500.0 / speed(w, k0.0, k0.1) + 500.0 / speed(w, k1.0, k1.1);
        let mut w = 250.0;
        while (w - 250.0) * even(w) < 10_000.0 {
            w += 0.5;
        }
        assert!(p.total_s as f32 <= even(w) + 1.0, "{} {}", p.total_s, even(w));
    }

    #[test]
    fn no_budget_is_cp_and_the_band_holds() {
        let secs = [Sec { len_m: 300, grade_x10: 50 }, Sec { len_m: 300, grade_x10: 120 }];
        let p = plan(&secs, &rider(0.0));
        assert_eq!(p.watts[..2], [250, 250]);
        let p = plan(&secs, &rider(1e9));
        assert!(p.watts[..2].iter().all(|&w| (250..=250 + 20_000 / 60).contains(&w)));
    }

    #[test]
    fn odd_inputs_stay_finite_and_bounded() {
        let odd = [
            Sec { len_m: 0, grade_x10: 0 },
            Sec { len_m: 65535, grade_x10: i16::MAX },
            Sec { len_m: 100, grade_x10: i16::MIN },
            Sec { len_m: 100, grade_x10: -80 },
        ];
        for mass in [0.0, -5.0, f32::NAN, 90.0] {
            for cp in [0.0, 1.0, 250.0, f32::NAN, f32::INFINITY] {
                let r = Rider { cp, w_full: 20_000.0, budget: 5_000.0, mass };
                let p = plan(&odd, &r);
                assert!(p.n as usize <= MAX && p.total_s < 1_000_000_000);
                if cp == 250.0 {
                    assert!(p.watts[..4].iter().all(|&w| (250..=584).contains(&w)), "{:?}", p.watts);
                }
            }
        }
        assert_eq!(plan(&[], &rider(1.0)), Plan::EMPTY);
        assert_eq!(plan(&[Sec::default(); 40], &rider(1.0)).n as usize, MAX);
    }

    #[test]
    fn replan_cost() {
        let secs: std::vec::Vec<Sec> =
            (0..32).map(|i| Sec { len_m: 100 + i * 5, grade_x10: 20 + (i as i16 * 37) % 100 }).collect();
        STEPS.with(|s| s.set(0));
        PASSES.with(|s| s.set(0));
        let p = plan(&secs, &rider(12_000.0));
        let (steps, passes) = (STEPS.with(|s| s.get()), PASSES.with(|s| s.get()));
        // about 40 cycles a Newton step and 32 x 15 cycles a pass, at 200 MHz
        let cycles = steps * 40 + passes * 32 * 15;
        std::println!("replan of 32 sections: {steps} Newton steps, {passes} passes, about {} us", cycles / 200);
        assert!(steps < 600 && passes < 30 && p.n == 32, "{steps} {passes}");
    }

    /// The cases the solver is compared on: (name, sections, budget J).
    fn cases() -> std::vec::Vec<(&'static str, std::vec::Vec<Sec>, f32)> {
        let rep = |n: usize, len: u16, g: i16| std::vec![Sec { len_m: len, grade_x10: g }; n];
        let mut stepped = rep(8, 500, 40);
        stepped.extend(rep(8, 500, 100));
        stepped.extend(rep(8, 500, 20));
        stepped.extend(rep(8, 500, 80));
        let mut dip = rep(10, 300, 80);
        dip.extend(rep(3, 150, -40));
        dip.extend(rep(10, 300, 90));
        let mixed: std::vec::Vec<Sec> =
            (0..32).map(|i| Sec { len_m: 100 + i * 5, grade_x10: 20 + (i as i16 * 37) % 100 }).collect();
        std::vec![
            ("col 10 km 7 %", rep(32, 312, 70), 15_000.0),
            ("stepped 4/10/2/8", stepped, 15_000.0),
            ("short descent", dip, 12_000.0),
            ("mixed", mixed, 12_000.0),
            ("two", std::vec![Sec { len_m: 500, grade_x10: 30 }, Sec { len_m: 500, grade_x10: 100 }], 10_000.0),
        ]
    }

    #[test]
    fn compare_cases() {
        for (name, secs, budget) in cases() {
            let p = plan(&secs, &rider(budget));
            let spent: f32 = secs
                .iter()
                .zip(&p.watts)
                .map(|(s, &w)| {
                    let (k, a) = coefs(s.grade_x10, 84.0);
                    (f32::from(w) - 250.0).max(0.0) * f32::from(s.len_m) / speed(f32::from(w), k, a)
                })
                .sum();
            assert!(spent < budget * 1.05 && p.total_s > 0, "{name}: {spent} of {budget}");
        }
    }

    #[test]
    fn speed_matches_the_power_equation() {
        let (k, a) = coefs(60, 84.0);
        let v = speed(300.0, k, a);
        assert!((v * (k + a * v * v) - 300.0 * ETA).abs() < 0.5);
        let (k, a) = coefs(-60, 84.0); // downhill: coasting speed at zero power
        assert!(speed(0.0, k, a) > 5.0);
    }
}
