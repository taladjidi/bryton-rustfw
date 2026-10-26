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
//! grade for sin(angle) and 1 for cos(angle) (3 % error at 20 %). Solved for `v` by bisection ([`speed`]).
//!
//! **Allocation.** Minimise the total time `sum t_i(P_i)` for `sum (P_i - CP) t_i <= W'`: a Lagrangian with
//! multiplier `lambda`, section by section `t'(1 + lambda (P - CP)) + lambda t = 0` (the marginal time saved per
//! joule of W' equal in every section): more power where the speed is low (the steep parts), as Swain 1997 ("A model
//! for optimizing cycling performance by varying power on hills and in wind", Med Sci Sports Exerc 29(8):1104-1108)
//! and the optimal-control studies find (Ashtiani, Vahidi et al., "Optimal pacing of a cyclist in a time trial based
//! on experimentally calibrated models of fatigue and recovery", arXiv 2007.11393: CP is one of the four optimal
//! modes, all-out another). `lambda` by bisection so the W' spent just fits the budget. On one constant grade this
//! gives `P = CP + W'/T`.
//!
//! The citations of Martin and Swain are from memory (the papers were not opened here); the arXiv one was checked.
//! Constants are those of a typical road rider climbing on the hoods; a setting later if the owner asks.

/// Sections a plan holds at most (the course's climbs are cut into at most this many).
pub const MAX: usize = 32;

/// Bike, bottles and kit, kg (added to the rider's weight).
pub const BIKE_KG: f32 = 9.0;
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
/// Upper bound of the multiplier, 1/W: above it every section is at CP (t'/t is about 1/(3 P)).
const LAMBDA_MAX: f32 = 0.1;

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

/// The steady speed at pedal power `p` W on a section with terms `k`, `a`, m/s: bisection on the wheel power, which
/// grows with speed above the speed where `k + a v^2` is zero (downhill) and from 0 otherwise.
pub fn speed(p: f32, k: f32, a: f32) -> f32 {
    let wheel = p.max(0.0) * ETA;
    let f = |v: f32| v * (k + a * v * v);
    let mut lo = if k < 0.0 { sqrt(-k / a).min(VMAX) } else { 0.0 };
    let mut hi = VMAX;
    if f(hi) <= wheel {
        return VMAX;
    }
    for _ in 0..20 {
        let mid = (lo + hi) * 0.5;
        if f(mid) < wheel {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    ((lo + hi) * 0.5).max(VMIN)
}

/// A section's time at `p`, s, and its derivative by power (s/W, never positive).
fn time(len: f32, k: f32, a: f32, p: f32) -> (f32, f32) {
    let v = speed(p, k, a);
    let dv = if v >= VMAX { 0.0 } else { ETA / (k + 3.0 * a * v * v).max(1e-3) };
    (len / v, -len / (v * v) * dv)
}

/// The power of one section for the multiplier `lam`, W, in `cp..=hi`.
fn power_at(len: f32, (k, a): (f32, f32), cp: f32, hi: f32, lam: f32) -> f32 {
    let slope = |p: f32| {
        let (t, dt) = time(len, k, a, p);
        dt * (1.0 + lam * (p - cp)) + lam * t
    };
    if slope(cp) >= 0.0 {
        return cp;
    }
    if slope(hi) <= 0.0 {
        return hi;
    }
    let (mut lo, mut up) = (cp, hi);
    for _ in 0..12 {
        let mid = (lo + up) * 0.5;
        if slope(mid) < 0.0 {
            lo = mid;
        } else {
            up = mid;
        }
    }
    (lo + up) * 0.5
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
    let mut sec = [(0f32, (0f32, 0f32)); MAX];
    for (s, i) in sec.iter_mut().zip(secs) {
        *s = (f32::from(i.len_m), coefs(i.grade_x10, mass));
    }
    let sec = sec.get(..n).unwrap_or(&[]);
    // the W' spent and the time for the multiplier `lam`
    let run = |lam: f32, out: Option<&mut [u16; MAX]>| {
        let (mut work, mut total) = (0f32, 0f32);
        let mut out = out;
        for (i, &(len, c)) in sec.iter().enumerate() {
            let p = if budget < 1.0 { cp } else { power_at(len, c, cp, hi, lam) };
            let (t, _) = time(len, c.0, c.1, p);
            (work, total) = (work + (p - cp) * t, total + t);
            if let Some(w) = out.as_deref_mut().and_then(|o| o.get_mut(i)) {
                *w = (p + 0.5).clamp(0.0, 65_535.0) as u16;
            }
        }
        (work, total)
    };
    let mut lam = 0.0;
    if budget >= 1.0 && run(0.0, None).0 > budget {
        let (mut lo, mut up) = (0.0, LAMBDA_MAX);
        for _ in 0..24 {
            let mid = (lo + up) * 0.5;
            if run(mid, None).0 > budget {
                lo = mid;
            } else {
                up = mid;
            }
        }
        lam = up;
    }
    let mut watts = [0u16; MAX];
    let (_, total) = run(lam, Some(&mut watts));
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
        let even = |w: f32| time(500.0, k0.0, k0.1, w).0 + time(500.0, k1.0, k1.1, w).0;
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
    fn speed_matches_the_power_equation() {
        let (k, a) = coefs(60, 84.0);
        let v = speed(300.0, k, a);
        assert!((v * (k + a * v * v) - 300.0 * ETA).abs() < 0.5);
        let (k, a) = coefs(-60, 84.0); // downhill: coasting speed at zero power
        assert!(speed(0.0, k, a) > 5.0);
    }
}
