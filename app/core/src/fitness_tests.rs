use super::*;

#[test]
fn days_are_counted_from_1970() {
    assert_eq!(day_of("1970-01-01"), Some(0));
    assert_eq!(day_of("2026-11-08"), Some(20_765));
    assert_eq!(day_of("2026-11-08T07:00:00"), Some(20_765));
    assert_eq!(day_of("2026-13-08"), None);
    // the month's own length, leap years included
    assert_eq!((day_of("2026-02-29"), day_of("2024-02-29").is_some(), day_of("2100-02-29")), (None, true, None));
    assert_eq!((day_of("2026-04-31"), day_of("2026-04-30").is_some()), (None, true));
    assert_eq!(day_of("junk"), None);
}

#[test]
fn ctl_and_atl_of_a_known_load() {
    // 100 on day 10, nothing after: x1 = 100/N, then x decays by (N-1)/N a day; TSB is yesterday's CTL - ATL
    let d = fitness_from_loads(vec![DayLoad { day: 10, load: 100 }], 12);
    assert_eq!(d.len(), 3);
    let (c0, a0) = (100.0 / 42.0, 100.0 / 7.0);
    assert!((d[0].ctl - c0).abs() < 1e-9 && (d[0].atl - a0).abs() < 1e-9);
    assert!((d[1].ctl - c0 * 41.0 / 42.0).abs() < 1e-9 && (d[1].atl - a0 * 6.0 / 7.0).abs() < 1e-9);
    assert!((d[1].tsb - (c0 - a0)).abs() < 1e-9);
    assert!((d[2].atl - a0 * 36.0 / 49.0).abs() < 1e-9);
    // loads in the future and none at all give nothing
    assert!(fitness_from_loads(vec![DayLoad { day: 20, load: 100 }], 12).is_empty());
    assert!(fitness_from_loads(vec![], 12).is_empty());
    // two rides in a day add; a long history keeps the last 90 days
    let many = fitness_from_loads(vec![DayLoad { day: 0, load: 50 }, DayLoad { day: 0, load: 50 }], 300);
    assert_eq!(many.len(), DAYS);
    assert_eq!(many.last().map(|d| d.day), Some(300));
}

#[test]
fn form_after_a_gap_is_the_days_own() {
    let j = r#"[{"id":"2026-11-01","ctl":50,"atl":60},{"id":"2026-11-02","ctl":51,"atl":58},{"id":"2026-11-05","ctl":52,"atl":50}]"#;
    let d = wellness_parse(j.as_bytes().to_vec()).unwrap().days;
    // day 2 starts with day 1's 50 - 60, day 5 (a gap of two days) with its own 52 - 50
    assert_eq!((d[1].tsb, d[2].tsb), (-10.0, 2.0));
}

#[test]
fn a_sprint_is_clamped_not_dropped() {
    let j = r#"{"list":[{"secs":[1,5],"values":[2500,1e9]}]}"#;
    let c = power_curve_parse(j.as_bytes().to_vec()).unwrap();
    assert_eq!(c, vec![CurvePoint { secs: 1, watts: 2500 }, CurvePoint { secs: 5, watts: 3000 }]);
    let w = wellness_parse(br#"[{"id":"2026-11-01","ctl":1,"atl":1,"sportInfo":[{"type":"Ride","eftp":5000}]}]"#.to_vec());
    assert_eq!(w.unwrap().eftp, Some(3000.0));
}

const SAMPLE: &str = r#"[
  {"id":"2026-11-01","ctl":50.5,"atl":60.2,"sportInfo":[{"type":"Run","eftp":999},{"type":"Ride","eftp":250.4}]},
  {"id":"2026-11-02T00:00:00","ctl":51,"atl":58,"rampRate":1.5},
  {"id":"2026-11-03","ctl":null,"atl":1},
  {"ctl":1,"atl":1},
  {"id":"2026-11-04","ctl":"x","atl":2},
  {"id":"2026-11-05","ctl":-3,"atl":2},
  {"id":"junk","ctl":1,"atl":1}
]"#;

#[test]
fn wellness_keeps_the_days_that_have_everything() {
    let w = wellness_parse(SAMPLE.as_bytes().to_vec()).unwrap();
    assert_eq!(w.days.len(), 2, "{:?}", w.days);
    assert_eq!((w.days[0].day, w.days[1].day), (20_758, 20_759));
    assert!((w.days[1].tsb - (50.5 - 60.2)).abs() < 1e-9);
    assert_eq!(w.eftp, Some(250.4));
    assert_eq!(wellness_parse(b"[]".to_vec()).unwrap(), Wellness::default());
    assert_eq!(wellness_parse(b"{}".to_vec()), Err(FitnessError::NotWellness));
    assert_eq!(wellness_parse(b"nope".to_vec()), Err(FitnessError::NotWellness));
}

#[test]
fn the_power_curve_is_read() {
    let j = r#"{"list":[{"days":90,"secs":[1,5,7,60,3600],"values":[900,800,700,400,0]}],"activities":{}}"#;
    let c = power_curve_parse(j.as_bytes().to_vec()).unwrap();
    assert_eq!(
        c,
        vec![CurvePoint { secs: 1, watts: 900 }, CurvePoint { secs: 5, watts: 800 }, CurvePoint { secs: 60, watts: 400 }]
    );
    assert!(power_curve_parse(b"{\"list\":[]}".to_vec()).is_err());
    assert!(power_curve_parse(b"{\"list\":[{\"secs\":[1]}]}".to_vec()).is_err());
}

/// Wellness documents made of the real structure with each field randomly right, wrong or missing, and the curve's.
#[test]
fn fuzz_the_answers() {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let (mut days, mut eftps, mut curves) = (0usize, 0usize, 0usize);
    for _ in 0..3000 {
        let mut doc = String::from("[");
        for k in 0..next(12) {
            let field = |name: &str, good: String, r: u64| match r {
                0 => String::new(),
                1 => format!("\"{name}\":null,"),
                2 => format!("\"{name}\":\"text\","),
                3 => format!("\"{name}\":-5,"),
                4 => format!("\"{name}\":1e30,"),
                _ => format!("\"{name}\":{good},"),
            };
            let id = field("id", format!("\"2026-0{}-1{}\"", 1 + next(9), next(9)), next(7));
            let (c, a) = (field("ctl", format!("{}", next(120)), next(7)), field("atl", format!("{}.5", next(120)), next(7)));
            let s = field("sportInfo", format!("[{{\"type\":\"Ride\",\"eftp\":{}}}]", 100 + next(200)), next(7));
            doc += &format!("{}{{{id}{c}{a}{s}\"z\":0}}", if k > 0 { "," } else { "" });
        }
        doc += "]";
        let w = wellness_parse(doc.into_bytes()).unwrap();
        assert!(w.days.windows(2).all(|p| p[0].day < p[1].day) && w.days.len() <= DAYS);
        days += w.days.len();
        eftps += usize::from(w.eftp.is_some());
        let secs: Vec<String> = (0..next(8)).map(|_| format!("{}", [1, 5, 60, 7, 3600][next(5) as usize])).collect();
        let vals: Vec<String> =
            (0..next(8)).map(|_| if next(5) == 0 { "null".into() } else { format!("{}", next(1200)) }).collect();
        let j = format!("{{\"list\":[{{\"secs\":[{}],\"values\":[{}]}}]}}", secs.join(","), vals.join(","));
        curves += power_curve_parse(j.into_bytes()).map_or(0, |c| c.len());
        // arbitrary bytes never panic either
        let junk: Vec<u8> = (0..next(40)).map(|_| next(256) as u8).collect();
        let _ = (wellness_parse(junk.clone()), power_curve_parse(junk));
    }
    // the generator must reach the accepting branches, or this tests nothing
    assert!(days > 200 && eftps > 100 && curves > 500, "{days} {eftps} {curves}");
}
