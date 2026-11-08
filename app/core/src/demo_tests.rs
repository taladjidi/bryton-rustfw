use super::*;

#[test]
fn demo_rides_follow_the_date_and_their_names_match_their_files() {
    assert_eq!(civil(0), (1970, 1, 1));
    assert_eq!(civil(20_400), (2025, 11, 8));
    let now = 1_794_177_000; // 2026-11-08 22:30 UTC, Sunday 23:30 in Paris
    let u = FakeUnit::demo_at(now);
    assert_eq!(u.rides.len(), 10);
    for (name, bytes) in &u.rides {
        let v = crate::ride::analyze_ride(bytes.clone(), 200, 190).unwrap();
        let local = v.summary.start_unix + 3600;
        let (y, m, d) = civil(local.div_euclid(86_400));
        let h = local.rem_euclid(86_400) / 3600;
        assert_eq!(String::from_utf8_lossy(name), format!("{:02}{m:02}{d:02}{h:02}0000", y % 100));
    }
    assert_eq!(&u.rides[0].0[..6], b"261108");
}
