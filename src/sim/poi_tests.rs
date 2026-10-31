//! Named points along the followed course: a synthetic card with stock-format tile files holding a peak and a col
//! near the synthetic hill (and a peak far from it), the scan once the course is on, the profile's markers, NEXT POI.

use super::course_tests::{course_route, hill};
use super::menu_tests::to_row;
use super::tests::run;
use super::tests::*;
use super::*;
use crate::fat::tests::{Ram, image};
use crate::ui::Field;

/// A tile's H and P files.
type Files = (std::vec::Vec<u8>, std::vec::Vec<u8>);

/// One tile's H and P files (STORAGE_AND_MAPS.md 4.5): `(class, grid x, grid y, name, elevation)` of layer 13.
pub(crate) fn tile(features: &[(u8, i16, i16, &str, u64)]) -> Files {
    let (mut h, mut p) = (std::vec::Vec::from((features.len() as u32).to_le_bytes()), std::vec::Vec::new());
    for &(class, x, y, name, ele) in features {
        h.extend_from_slice(&[13, 1, 0]);
        h.extend_from_slice(&(p.len() as u32).to_le_bytes());
        h.extend_from_slice(&[class, 0, 0]);
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
        p.push(name.len() as u8);
        p.extend_from_slice(name.as_bytes());
        p.push(0);
        p.extend_from_slice(&ele.to_le_bytes());
    }
    (h, p)
}

/// The grid position (x, y) of a point `east` and `north` metres from 0 N 0 E in its zoom-11 tile.
fn grid(east: f64, north: f64) -> (i16, i16) {
    grid_at(11, east, north)
}

/// The same in a tile of zoom `z`.
fn grid_at(z: u32, east: f64, north: f64) -> (i16, i16) {
    let (lon, phi) = (east / 111_320.0, (north / 111_320.0).to_radians());
    let n = f64::from(1u32 << z);
    let tx = (lon + 180.0) / 360.0 * n;
    let ty = (1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / core::f64::consts::PI) / 2.0 * n;
    ((tx.fract() * 4096.0).round() as i16, (ty.fract() * 4096.0).round() as i16)
}

/// Writes tiles `(name, (H, P))` into `\Map\Data\Temp\Zoom<z>\H|P` of `image`.
pub(crate) fn with_tiles(image: std::vec::Vec<u8>, z: &str, tiles: &[(&str, Files)]) -> std::vec::Vec<u8> {
    let mut d = Ram(image, 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let t = crate::fat::write::dos_time((2026, 10, 31, 12, 0, 0));
    let mut dir = fat.root();
    for part in ["Map", "Data", "Temp", z] {
        dir = fat.ensure_dir(&mut d, dir, part, t).unwrap();
    }
    for (hp, pick) in [("H", 0), ("P", 1)] {
        let sub = fat.ensure_dir(&mut d, dir, hp, t).unwrap();
        for (name, (h, p)) in tiles {
            let mut bytes = if pick == 0 { h.clone() } else { p.clone() };
            let len = bytes.len() as u32;
            bytes.resize(bytes.len().div_ceil(crate::fat::SECTOR) * crate::fat::SECTOR, 0);
            let Ok(mut f) = fat.create(&mut d, sub, name, t) else { continue }; // the image has this zoom-12 tile (roads) already
            fat.write_sectors(&mut d, &mut f, 0, &bytes).unwrap();
            fat.set_size(&mut d, &mut f, len, t).unwrap();
        }
    }
    d.0
}

/// The test volume with zoom-11 tiles beside the hill: a col 60 m off its first leg, a peak 50 m off its climb, and
/// a peak 1.5 km east of it.
pub(super) fn image_with_names() -> std::vec::Vec<u8> {
    let (col, peak, far) = (grid(-1000.0, -1440.0), grid(50.0, -500.0), grid(1500.0, -500.0));
    let tiles = [
        ("1023_1024_0.bin", tile(&[(3, col.0, col.1, "Col de Test", 1333)])),
        ("1024_1024_0.bin", tile(&[(1, peak.0, peak.1, "Pic d'Essai", 1777), (1, far.0, far.1, "Far Peak", 3000)])),
    ];
    with_tiles(image(), "Zoom11", &tiles)
}

/// Following the hill: the scan finds the col and the peak (not the far one), in order along the course; NEXT POI
/// counts down to the next, and the profile page marks both and names the next in its title.
#[test]
fn names_along_the_hill() {
    use Field::*;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = image_with_names();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some((0, 0)) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(course_route);
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    app.set_course_memory(crate::storage::course_file::tests::memory());
    app.set_poi_memory(crate::named::memory());
    app.pages[0] = PageCfg { layout: 3, fields: [Profile, NextPoi, ClimbGain, Time, Time, Time] }.into();
    let text = |app: &App| std::string::String::from(app.field_text(NextPoi).as_str());
    assert_eq!(text(&app), "--", "no course");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    let names: std::vec::Vec<_> = {
        let p = app.pois.as_deref().unwrap();
        assert!(p.done(), "the scan ended");
        p.list().iter().map(|p| (p.name(), p.ele_m, p.col, p.along_m / 500, p.off_m / 25)).collect()
    };
    assert_eq!(names, [("COL DE TEST", 1333, true, 2, 2), ("PIC D'ESSAI", 1777, false, 5, 1)], "{names:?}");
    // before a fix: from the start
    assert!(["1.0K", "0.9K"].contains(&text(&app).as_str()), "{}", text(&app));
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    run(&mut sim, &mut app, 150_000);
    assert!(["1.7K", "1.8K", "1.9K"].contains(&text(&app).as_str()), "past the col: {}", text(&app));
    run(&mut sim, &mut app, 3000);
    assert_consistent(&sim, &app);
    let cyan = sim.screen.iter().filter(|&&p| p == ui::CYAN).count();
    assert!(cyan > 20, "the markers are drawn: {cyan} cyan pixels");
    sim.save("course_poi");
}

/// Random but structured tiles (names of every length, bytes that are not UTF-8, a missing terminator, records
/// pointing past P, other layers and classes, a wrong H size) beside a made-up line: the scan always ends, keeps at
/// most `MAX` points in order, and the fuzz reaches the matching over the seeds.
#[test]
fn fuzz_named_points() {
    use crate::named::{MAX, Pois, parse_point};
    let (mut found, mut parsed, mut full, mut long) = (0usize, 0usize, 0usize, 0);
    for seed in 1..=24u32 {
        let mut s = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move |n: u32| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s % n
        };
        let mut course = crate::course::Course::new();
        // 111 m south of the equator, 55 m a step; every fourth seed 400 points (several collecting passes)
        let points = if seed % 4 == 0 { 400 } else { 80 };
        for k in 0..=points {
            course.push(-10_000, -200_000 + k * 5000 * 80 / points, None);
        }
        course.finish();
        let mut image = image();
        for (z, tx, ty) in
            [("Zoom12", [2047u32, 2048], 2048), ("Zoom11", [1023, 1024], 1024), ("Zoom9", [255, 256], 256)]
        {
            let mut tiles = std::vec::Vec::new();
            for x in tx {
                let (mut h, mut p) = (std::vec::Vec::new(), std::vec::Vec::<u8>::new());
                let n = 100 + next(300);
                h.extend_from_slice(&n.to_le_bytes());
                for _ in 0..n {
                    let (layer, class) = ([13, 13, 13, 13, 5, 7, 1][next(7) as usize], next(6) as u8);
                    let off = if next(10) == 0 { next(100_000) } else { p.len() as u32 };
                    h.extend_from_slice(&[layer, [1, 1, 1, 0, 2][next(5) as usize], 0]);
                    h.extend_from_slice(&off.to_le_bytes());
                    h.extend_from_slice(&[class, 0, 0]);
                    let east = f64::from(next(2500)) * if x % 2 == 0 { 1.0 } else { -1.0 }; // the east tile or the west one
                    let north = -111.0 + f64::from(next(300)) - 150.0;
                    let (gx, gy) = grid_at(z[4..].parse().unwrap(), east, north);
                    let (gx, gy) = if next(8) == 0 { (next(65536) as i16, next(65536) as i16) } else { (gx, gy) };
                    p.extend_from_slice(&gx.to_le_bytes());
                    p.extend_from_slice(&gy.to_le_bytes());
                    let len = if next(10) == 0 { next(250) as u8 } else { next(30) as u8 };
                    p.push(len);
                    p.extend((0..len).map(|_| if next(6) == 0 { next(256) as u8 } else { b'a' + next(26) as u8 }));
                    if next(12) != 0 {
                        p.push(0);
                    }
                    p.extend_from_slice(&(u64::from(next(u32::MAX)) << next(33)).to_le_bytes());
                }
                if next(15) == 0 {
                    h.push(0); // a wrong size
                }
                tiles.push((std::format!("{x}_{ty}_0.bin"), (h, p)));
            }
            let tiles: std::vec::Vec<_> = tiles.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
            image = with_tiles(image, z, &tiles);
            for (_, (_, p)) in &tiles {
                for at in (0..p.len()).step_by(7) {
                    parsed += usize::from(parse_point(p.get(at..).unwrap_or(&[])).is_some());
                }
            }
        }
        let mut d = Ram(image, 0);
        let mut pois = Pois::new();
        let mut passes = 0;
        if points > 80 {
            pois.step(&course, &mut d);
            assert!(!pois.done(), "seed {seed}: more than one collecting pass");
            long += 1;
        }
        while !pois.done() && passes < 2000 {
            pois.step(&course, &mut d);
            passes += 1;
        }
        assert!(pois.done(), "seed {seed}: the scan ends ({passes} passes)");
        let l = pois.list();
        assert!(l.len() <= MAX && l.windows(2).all(|w| w[0].along_m <= w[1].along_m), "seed {seed}: ordered, bounded");
        assert!(
            l.iter().all(|p| u32::from(p.off_m) <= crate::named::NEAR_M && p.name().is_ascii() && !p.name().is_empty())
        );
        found += l.len();
        full += usize::from(l.len() == MAX);
    }
    std::println!("named fuzz: {found} points kept, {parsed} records parsed, {full} full lists");
    assert!(
        found > 100 && parsed > 500 && full >= 1 && long >= 3,
        "the fuzz reaches the matching: {found}, {parsed}, {full}"
    );
}
