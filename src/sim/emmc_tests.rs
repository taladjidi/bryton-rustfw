//! The eMMC driver (emmc.rs) against the card model, and the dev link's `sector` and `emmc` commands.

use super::*;
use crate::dev::{Effect, Session, Stats, send_layout, send_sector};
use crate::emmc::{BusError, Card, Emmc, Error, Layout, SECTOR};

/// The FAT32 volume macOS made (fat.rs tests), read through the driver and the card model: files equal the
/// tiles, and the reads of a file are multi-block (CMD18) where whole sectors go to the buffer.
#[test]
fn reads_files_from_the_fat_volume() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut emmc = Emmc::new();
    let mut d = crate::emmc::Volume(&mut emmc, &mut sim);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, "Map/Data/Temp/Zoom14/P/8192_8192_0.bin").unwrap();
    let mut buf = vec![0u8; f.size as usize];
    assert_eq!(fat.read_at(&mut d, &f, 0, &mut buf), Ok(buf.len()));
    let h = fat.open(&mut d, "map/data/temp/zoom14/h/8192_8192_0.bin").unwrap();
    let mut n = [0u8; 4];
    fat.read_at(&mut d, &h, 0, &mut n).unwrap();
    assert_eq!(u32::from_le_bytes(n) * 10 + 4, h.size, "an H file: count, then 10 bytes a feature");
    // the same bytes as the image read directly
    let mut ram = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
    let mut direct = vec![0u8; f.size as usize];
    crate::fat::Fat::mount(&mut ram).unwrap().read_at(&mut ram, &f, 0, &mut direct).unwrap();
    assert_eq!(buf, direct);
    let frag = fat.open(&mut d, "frag.bin").unwrap(); // 200 KB: whole clusters
    let mut big = vec![0u8; frag.size as usize];
    assert_eq!(fat.read_at(&mut d, &frag, 0, &mut big), Ok(big.len()));
    assert!(sim.emmc.cmds.iter().any(|c| c.0 == 18), "multi-block reads");
    assert!(sim.emmc.cmds.iter().all(|c| [0, 1, 2, 3, 6, 7, 8, 12, 13, 16, 17, 18].contains(&c.0)));
}

/// Writes: CMD24 for one sector, CMD25 + CMD12 for more, CMD13 after each (the card programming); nothing at
/// or past B goes out at all; a card that loses power mid-write is an error, and the next use starts from CMD0.
#[test]
fn writes_below_b_only() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = vec![0; 64 * SECTOR];
    let card = Card::init(&mut sim).unwrap();
    sim.emmc.cmds.clear();
    card.write(&mut sim, 3, &[7u8; SECTOR]).unwrap();
    card.write(&mut sim, 8, &[9u8; 4 * SECTOR]).unwrap();
    assert_eq!(
        sim.emmc.cmds,
        [(13, 0x1_0000), (24, 3), (13, 0x1_0000), (13, 0x1_0000), (25, 8), (12, 0), (13, 0x1_0000)]
    );
    assert!(sim.emmc.image[3 * SECTOR..4 * SECTOR].iter().all(|&b| b == 7));
    assert!(sim.emmc.image[8 * SECTOR..12 * SECTOR].iter().all(|&b| b == 9));
    let mut back = [0u8; 4 * SECTOR];
    card.read(&mut sim, 8, &mut back).unwrap();
    assert!(back.iter().all(|&b| b == 9));
    sim.emmc.cmds.clear();
    let b = card.layout.fat_end;
    for (lba, n) in [(b, 1), (b - 1, 2), (b - 2, 4), (u32::MAX, 1)] {
        assert_eq!(card.write(&mut sim, lba, &vec![0u8; n * SECTOR]), Err(Error::Range), "{lba:#x} + {n}");
    }
    assert_eq!(card.write(&mut sim, 0, &[0u8; 100]), Err(Error::Range), "not whole sectors");
    assert!(sim.emmc.cmds.is_empty(), "refused before any command");
    // a build that may not write the card: refused here, whoever the writer, nothing sent
    sim.writable = false;
    assert_eq!(card.write(&mut sim, 3, &[1u8; SECTOR]), Err(Error::ReadOnly));
    assert_eq!(Emmc::new().write(&mut sim, 3, &[1u8; SECTOR]), Err(Error::ReadOnly));
    assert!(sim.emmc.cmds.iter().all(|c| !matches!(c.0, 24 | 25)), "no write command");
    assert!(sim.emmc.image[3 * SECTOR..4 * SECTOR].iter().all(|&b| b == 7));
    sim.writable = true;
    // the power goes after 2 sectors of a 4-sector write: the first 2 land, the card is gone
    let mut emmc = Emmc::new();
    sim.emmc.cut_after = Some(sim.emmc.sectors_written + 2);
    assert!(emmc.write(&mut sim, 20, &[5u8; 4 * SECTOR]).is_err());
    assert!(sim.emmc.image[20 * SECTOR..22 * SECTOR].iter().all(|&b| b == 5));
    assert!(sim.emmc.image[22 * SECTOR..24 * SECTOR].iter().all(|&b| b == 0));
}

/// Reliable write when the card offers it (EXT_CSD 166 / 222, emmc.rs): CMD23 with bit 31 and the count, then
/// CMD25 and no CMD12; legacy, one sector at a time. A cut in a plain write tears the sector being written; in a
/// reliable one, the sector keeps its old data.
#[test]
fn reliable_writes_when_offered() {
    let w = |rel_param, rel_sectors| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = vec![0; 64 * SECTOR];
        (sim.emmc.rel_param, sim.emmc.rel_sectors) = (rel_param, rel_sectors);
        let card = Card::init(&mut sim).unwrap();
        sim.emmc.cmds.clear();
        card.write(&mut sim, 3, &[7u8; SECTOR]).unwrap();
        card.write(&mut sim, 8, &[9u8; 2 * SECTOR]).unwrap();
        assert!(sim.emmc.image[8 * SECTOR..10 * SECTOR].iter().all(|&b| b == 9));
        sim.emmc.cmds.into_iter().filter(|c| c.0 != 13).collect::<Vec<_>>()
    };
    let rel = 1 << 31;
    assert_eq!(w(crate::emmc::EN_REL_WR, 1), [(23, rel | 1), (25, 3), (23, rel | 2), (25, 8)]);
    assert_eq!(w(0, 1), [(23, rel | 1), (25, 3), (23, rel | 1), (25, 8), (23, rel | 1), (25, 9)]);
    assert_eq!(w(0, 0), [(24, 3), (25, 8), (12, 0)]);
    for (rel_param, torn) in [(0, true), (crate::emmc::EN_REL_WR, false)] {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = vec![1; 64 * SECTOR];
        (sim.emmc.rel_param, sim.emmc.tear) = (rel_param, true);
        let mut emmc = Emmc::new();
        sim.emmc.cut_after = Some(sim.emmc.sectors_written + 1);
        assert!(emmc.write(&mut sim, 20, &[5u8; 2 * SECTOR]).is_err());
        assert!(sim.emmc.image[20 * SECTOR..21 * SECTOR].iter().all(|&b| b == 5));
        let second = &sim.emmc.image[21 * SECTOR..22 * SECTOR];
        assert_eq!(!second.iter().all(|&b| b == 1), torn, "rel_param {rel_param}");
    }
}

/// Identification from the state the bootloader leaves (selected, 4-bit), without stock's SD probes, then the
/// layout from SEC_COUNT.
#[test]
fn init_finds_the_layout() {
    let mut sim = Sim::new(9, 0, 0);
    let t0 = sim.ms;
    let card = Card::init(&mut sim).unwrap();
    assert!(sim.ms - t0 < 20, "init took {} ms", sim.ms - t0);
    assert_eq!(card.layout, Layout { sectors: EMMC_SECTORS, fat_end: 0x03a2_bfe6, os_start: 0x03a3_b000 });
    let order: Vec<u8> = sim.emmc.cmds.iter().map(|c| c.0).collect();
    assert_eq!(order, [0, 1, 1, 1, 1, 2, 3, 7, 13, 16, 8, 6, 13, 8]);
    assert!(sim.emmc.cmds.contains(&(1, 0x40ff_8000)) && sim.emmc.cmds.contains(&(3, 0x1_0000)));
    assert!(sim.emmc.host_wide && sim.emmc.card_wide && sim.emmc.host_fast);
}

#[test]
fn reads_single_and_multiple_sectors() {
    let mut sim = Sim::new(9, 0, 0);
    let card = Card::init(&mut sim).unwrap();
    let l = card.layout;
    let mut one = [0u8; SECTOR];
    for lba in [0, 1, l.fat_end, l.os_start, EMMC_SECTORS - 1] {
        card.read(&mut sim, lba, &mut one).unwrap();
        assert_eq!(one, emmc_sector(lba), "sector {lba:#x}");
    }
    let mut eight = [0u8; 8 * SECTOR];
    sim.emmc.cmds.clear();
    card.read(&mut sim, EMMC_SECTORS - 8, &mut eight).unwrap(); // up to the last sector
    for (i, s) in eight.chunks_exact(SECTOR).enumerate() {
        assert_eq!(s, emmc_sector(EMMC_SECTORS - 8 + i as u32));
    }
    assert_eq!(sim.emmc.cmds, [(13, 0x1_0000), (18, EMMC_SECTORS - 8), (12, 0)]);
    // nothing past the end, nothing that is not whole sectors: refused before any command
    sim.emmc.cmds.clear();
    assert_eq!(card.read(&mut sim, EMMC_SECTORS - 7, &mut eight), Err(Error::Range));
    assert_eq!(card.read(&mut sim, u32::MAX, &mut one), Err(Error::Range));
    assert_eq!(card.read(&mut sim, 0, &mut one[..100]), Err(Error::Range));
    assert_eq!(card.read(&mut sim, 0, &mut []), Err(Error::Range));
    assert!(sim.emmc.cmds.is_empty());
    // the card still works after them
    card.read(&mut sim, 5, &mut one).unwrap();
    assert_eq!(one, emmc_sector(5));
}

/// A card that never answers fails at once; one that stays busy, after the 1 s JEDEC allows.
#[test]
fn a_silent_or_busy_card_costs_bounded_time() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.dead = true;
    let t0 = sim.ms;
    assert_eq!(Card::init(&mut sim), Err(Error::Bus(1, BusError::Timeout)));
    assert!(sim.ms - t0 < 10);
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.busy_polls = u32::MAX;
    let t0 = sim.ms;
    assert_eq!(Card::init(&mut sim), Err(Error::Busy));
    assert!(sim.ms - t0 <= 1010, "{} ms", sim.ms - t0);
}

/// After a bus error the card is forgotten and identified again from CMD0 at the next read.
#[test]
fn identifies_again_after_an_error() {
    let mut sim = Sim::new(9, 0, 0);
    let (mut emmc, mut buf) = (Emmc::new(), [0u8; SECTOR]);
    emmc.read(&mut sim, 7, &mut buf).unwrap();
    sim.emmc.dead = true;
    assert_eq!(emmc.read(&mut sim, 7, &mut buf), Err(Error::Bus(13, BusError::Timeout)));
    sim.emmc.dead = false;
    sim.emmc.cmds.clear();
    emmc.read(&mut sim, 8, &mut buf).unwrap();
    assert_eq!((sim.emmc.cmds[0], buf), ((0, 0), emmc_sector(8)));
}

/// The eMMC is not touched at boot (past the first start, which imports stock's settings), only by the dev link; a
/// dead card answers an error, the app runs on, and the card is identified again once it answers.
#[test]
fn sector_command_reads_and_survives_a_dead_card() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    assert!(sim.emmc.cmds.is_empty(), "the eMMC was touched at boot");
    let (mut session, mut pc, mut emmc) = (Session::new(), Pc::default(), Emmc::new());
    let mut serve = |sim: &mut Sim, session: &mut Session, pc: &mut Pc, line: &[u8]| {
        pc.got.clear();
        match session.feed(line, &app, Stats::default(), pc) {
            Effect::Sector(n) => {
                let mut buf = [0u8; SECTOR];
                let r = emmc.read(sim, n, &mut buf).map(|()| &buf);
                send_sector(pc, r);
            }
            Effect::Emmc => send_layout(pc, emmc.card(sim)),
            Effect::WriteTest(n) => crate::dev::write_test(pc, &mut emmc, sim, n),
            e => panic!("{e:?}"),
        }
    };
    sim.emmc.dead = true;
    serve(&mut sim, &mut session, &mut pc, b"sector 0\n");
    assert_eq!(pc.got, b"err cmd 1 timeout\n");
    sim.emmc.dead = false;
    serve(&mut sim, &mut session, &mut pc, b"emmc\n");
    assert_eq!(pc.got, b"emmc 03a3e000 03a2bfe6 03a3b000 00000000\n");
    serve(&mut sim, &mut session, &mut pc, b"sector 0x03a3b000\n");
    let hex: std::string::String = emmc_sector(0x03a3_b000).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(pc.got, format!("{hex}\n").as_bytes());
    serve(&mut sim, &mut session, &mut pc, b"sector 0x03a3e000\n");
    assert_eq!(pc.got, b"err range\n");
    // the write test leaves the sector as it was; past B it is refused at the first write
    sim.emmc.image = (0..0x1001 * 512).map(|i| (i * 7 % 251) as u8).collect();
    let before = sim.emmc.image.clone();
    serve(&mut sim, &mut session, &mut pc, b"wtest 0x1000\n");
    assert_eq!(pc.got, b"ok\n");
    assert!(sim.emmc.image == before, "the sector is restored");
    assert_eq!(sim.emmc.cmds.iter().filter(|c| matches!(c.0, 24 | 25)).count(), 2, "written twice");
    serve(&mut sim, &mut session, &mut pc, b"wtest 0x03a2bfe6\n");
    assert_eq!(pc.got, b"err write\n");
    // never while a ride is open: the recorder is the card's only writer then
    app.press(&mut sim, Press::Short(Key::Start));
    pc.got.clear();
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(session.feed(b"wtest 0x1000\n", &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err ride open\n");
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Start));
    pc.got.clear();
    assert_eq!(session.feed(b"sector\nsector 1 2\n", &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err usage: sector <n>\nerr usage: sector <n>\n");
    let t = sim.ms;
    app.step(&mut sim); // the app goes on
    sim.delay_ms(1000);
    app.step(&mut sim);
    assert!(sim.ms > t && sim.visible());
}
