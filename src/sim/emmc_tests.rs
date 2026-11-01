//! The eMMC driver (emmc.rs) against the card model, and the dev link's `sector` and `emmc` commands.

use super::*;
use crate::dev::{Effect, Session, Stats, send_layout, send_sector};
use crate::emmc::{BACKOFF_FIRST_MS, BusError, Card, Emmc, Error, Layout, SECTOR};

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
        emmc.advance(BACKOFF_FIRST_MS); // time passes between commands: the wait after a failed identification runs out
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
    pc.got.clear();
    assert_eq!(session.feed(b"sector\nsector 1 2\n", &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err usage: sector <n>\nerr usage: sector <n>\n");
    // never while a ride is open: the recorder is the card's only writer then
    app.press(&mut sim, Press::Short(Key::Start));
    pc.got.clear();
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(session.feed(b"wtest 0x1000\n", &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err ride open\n");
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    let t = sim.ms;
    app.step(&mut sim); // the app goes on
    sim.delay_ms(1000);
    app.step(&mut sim);
    assert!(sim.ms > t && sim.visible());
}

/// A card that stays busy at CMD1: asked every second for 130 s, the app's `Emmc` identifies it once per backoff
/// period (5, 10, 20, 40 s), not once per ask, and the refusals cost no time.
#[test]
fn a_busy_card_is_identified_once_per_backoff_period() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.busy_polls = u32::MAX;
    let (mut emmc, mut buf) = (Emmc::new(), [0u8; SECTOR]);
    let (mut spent, mut refused) = (0, 0);
    for _ in 0..130 {
        let t0 = sim.ms;
        match emmc.read(&mut sim, 7, &mut buf) {
            Err(Error::Backoff) => refused += 1,
            Err(Error::Busy) => {}
            other => panic!("{other:?}"),
        }
        spent += sim.ms - t0;
        emmc.advance(1000);
    }
    let inits = sim.emmc.cmds.iter().filter(|c| c.0 == 0).count();
    assert_eq!(inits, 5, "at 0, 5, 15, 35 and 75 s");
    assert_eq!(refused, 125);
    assert!(spent <= 5 * 1010, "{spent} ms spent");
    // the card comes back: identified at the end of the wait
    sim.emmc.busy_polls = 0;
    for _ in 0..70 {
        if emmc.read(&mut sim, 7, &mut buf).is_ok() {
            break;
        }
        emmc.advance(1000);
    }
    assert_eq!(buf, emmc_sector(7));
}

/// A card that identifies fine but fails its reads: asked every second for 130 s, it is identified (and the read
/// tried) once per backoff period, not at every ask.
#[test]
fn a_card_that_fails_its_reads_costs_one_try_per_backoff_period() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.unreadable = std::vec![0..u32::MAX];
    let (mut emmc, mut buf) = (Emmc::new(), [0u8; SECTOR]);
    for _ in 0..130 {
        let r = emmc.read(&mut sim, 7, &mut buf);
        assert!(r.is_err());
        emmc.advance(1000);
    }
    let inits = sim.emmc.cmds.iter().filter(|c| c.0 == 0).count();
    assert_eq!(inits, 5, "at 0, 5, 15, 35 and 75 s");
    sim.emmc.unreadable.clear();
    emmc.retry_now();
    emmc.read(&mut sim, 7, &mut buf).unwrap();
    assert_eq!(buf, emmc_sector(7));
}

/// A card that worked (one good read) and then fails every read, asked once a second for 130 s: identified six times
/// (the first, then the immediate retry of a card that had worked, then after 5, 10, 20 and 40 s waits). It pins
/// that `proven` is cleared by a failure, so a degraded card is not identified again at every pass.
#[test]
fn a_card_that_worked_and_then_fails_is_identified_six_times_in_130_s() {
    let mut sim = Sim::new(9, 0, 0);
    let (mut emmc, mut buf) = (Emmc::new(), [0u8; SECTOR]);
    emmc.read(&mut sim, 7, &mut buf).unwrap();
    sim.emmc.unreadable = std::vec![0..u32::MAX];
    for _ in 0..130 {
        assert!(emmc.read(&mut sim, 7, &mut buf).is_err());
        emmc.advance(1000);
    }
    assert_eq!(sim.emmc.cmds.iter().filter(|c| c.0 == 0).count(), 6);
}

/// The card model's rules (emmc_model.rs) trigger: an illegal command is not answered and the next answer says
/// ILLEGAL_COMMAND, once (E2); a write to a protected sector lands nothing and the next answer says WP_VIOLATION
/// (E2); a read past the card is OUT_OF_RANGE; a BKOPS stall answers CMD13 programming with EXCEPTION_EVENT, refuses
/// a read meanwhile and ends on time (E1, E4); a CMD0 does not end the busy (CMD1 waits it out).
#[test]
fn the_card_models_rules_trigger() {
    use super::emmc_model::{EXCEPTION_EVENT, ILLEGAL_COMMAND, OUT_OF_RANGE, WP_VIOLATION};
    use crate::Board;
    use crate::emmc::Resp::R1;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = vec![0; 64 * SECTOR];
    let card = Card::init(&mut sim).unwrap();
    let status = |sim: &mut Sim| sim.emmc_cmd(13, 1 << 16, R1).unwrap()[0];
    assert_eq!(sim.emmc_cmd(12, 0, R1), Err(BusError::Timeout), "CMD12 in tran: not answered");
    assert_eq!(status(&mut sim) & ILLEGAL_COMMAND, ILLEGAL_COMMAND, "said in the next answer");
    assert_eq!(status(&mut sim) & ILLEGAL_COMMAND, 0, "and cleared by it");
    sim.emmc.wp.push(5..6);
    assert_eq!(sim.emmc_write(24, 5, &[3; SECTOR]).map(|s| s & WP_VIOLATION), Ok(0), "the command's answer is clean");
    assert!(sim.emmc.image[5 * SECTOR..6 * SECTOR].iter().all(|&b| b == 0), "nothing landed");
    assert_eq!(status(&mut sim) & WP_VIOLATION, WP_VIOLATION);
    assert_eq!(status(&mut sim) & WP_VIOLATION, 0, "clear by read");
    let mut buf = [0u8; SECTOR];
    assert_eq!(sim.emmc_read(17, EMMC_SECTORS, &mut buf), Err(BusError::Timeout));
    assert_eq!(status(&mut sim) & OUT_OF_RANGE, OUT_OF_RANGE);
    sim.emmc.bkops(sim.ms, 40);
    let s = status(&mut sim);
    assert_eq!((s >> 9 & 0xf, s & 1 << 8, s & EXCEPTION_EVENT), (7, 0, EXCEPTION_EVENT), "{s:#x}");
    assert_eq!(sim.emmc_read(17, 3, &mut buf), Err(BusError::Timeout), "a read while programming: illegal");
    let t0 = sim.ms;
    assert_eq!(card.read(&mut sim, 3, &mut buf), Ok(()), "the driver waits the stall out first");
    assert!((40..60).contains(&(sim.ms - t0 + 1)), "{} ms", sim.ms - t0);
    sim.emmc.bkops(sim.ms, 30);
    let t0 = sim.ms;
    assert!(Card::init(&mut sim).is_ok());
    assert!(sim.ms - t0 >= 30, "CMD1 busy until the programming is over: {} ms", sim.ms - t0);
}

/// The R1 error bits fail the write where the card reports them (emmc_model.rs `DATA_ERRORS`, `COMMAND_ERRORS`):
/// - the data bits in the first CMD13 after the write or its stop, also while still programming (clear by read: a
///   poll that drops them loses them; Linux's `mmc_blk_busy_cb` accumulates them), and in the stop's answer after an
///   open-ended CMD25 (Linux checks `brq->stop.resp[0]`), but OUT_OF_RANGE there (`oor_with_open_end`);
/// - every bit in the write command's own answer;
/// - a command bit (COM_CRC_ERROR, ILLEGAL_COMMAND: the previous command's) in a poll while programming is not the
///   write's error.
///
/// Plain or reliable, one sector or four. EXCEPTION_EVENT alone is not an error.
#[test]
fn every_r1_error_bit_fails_the_write() {
    use super::emmc_model::{COMMAND_ERRORS, DATA_ERRORS, EmmcCard, OUT_OF_RANGE};
    let write = |rel, polls, sectors, set: &dyn Fn(&mut EmmcCard)| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = vec![0; 64 * SECTOR];
        sim.emmc.rel_param = rel;
        let card = Card::init(&mut sim).unwrap();
        sim.emmc.prog_polls = polls;
        set(&mut sim.emmc);
        card.write(&mut sim, 8, &vec![1; sectors * SECTOR])
    };
    let rel = crate::emmc::EN_REL_WR;
    let all = [(0, 0, 1), (0, 3, 1), (0, 0, 4), (0, 3, 4), (rel, 0, 4), (rel, 3, 4)];
    let fails = |got: Result<(), Error>, bit| matches!(got, Err(Error::Status(_, s)) if s & bit == bit);
    for (bit, name) in DATA_ERRORS {
        for (rel, polls, sectors) in all {
            let got = write(rel, polls, sectors, &|c| c.prog_error = bit);
            assert!(fails(got, bit), "{name} programming: {got:?} (reliable {rel}, {polls} polls, {sectors} sectors)");
        }
        for polls in [0, 3] {
            let got = write(0, polls, 4, &|c| c.stop_error = bit);
            assert_eq!(fails(got, bit), bit != OUT_OF_RANGE, "{name} in the stop: {got:?} ({polls} polls)");
        }
    }
    for (bit, name) in DATA_ERRORS.iter().chain(&COMMAND_ERRORS) {
        for (rel, polls, sectors) in all {
            let got = write(rel, polls, sectors, &|c| c.write_answer = *bit);
            assert!(fails(got, *bit), "{name} in the write's answer: {got:?} (reliable {rel}, {sectors} sectors)");
        }
    }
    for (bit, name) in COMMAND_ERRORS {
        for (rel, sectors) in [(0, 1), (0, 4), (rel, 4)] {
            let got = write(rel, 3, sectors, &|c| c.prog_error = bit);
            assert_eq!(got, Ok(()), "{name} in a poll while programming (reliable {rel}, {sectors} sectors)");
        }
    }
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = vec![0; 64 * SECTOR];
    let card = Card::init(&mut sim).unwrap();
    sim.emmc.bkops(sim.ms, 20);
    assert_eq!(card.write(&mut sim, 8, &[1; 2 * SECTOR]), Ok(()), "URGENT_BKOPS raised: a status, not an error");
}

/// A protected sector: the write lands nothing and the driver says so, a single sector or four, on a plain card
/// (where the violation comes in CMD12's answer) or a reliable one.
#[test]
fn a_write_protect_violation_is_an_error() {
    for (rel, sectors) in [(0, 1), (0, 4), (crate::emmc::EN_REL_WR, 4)] {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = vec![0; 64 * SECTOR];
        sim.emmc.rel_param = rel;
        let card = Card::init(&mut sim).unwrap();
        sim.emmc.wp.push(8..9);
        let got = card.write(&mut sim, 8, &vec![1; sectors * SECTOR]);
        assert!(
            matches!(got, Err(Error::Status(_, s)) if s & super::emmc_model::WP_VIOLATION != 0),
            "{got:?} (reliable {rel}, {sectors} sectors)"
        );
        assert!(sim.emmc.image.iter().all(|&b| b == 0), "nothing landed");
    }
}

/// The card's stall (emmc_model.rs E1) under the driver's 1 s bound is waited out (asleep: the clock moves, the CPU
/// does not work through it); over it, the driver gives up (`Busy`) with the data on the card, and the next use
/// identifies the card again once the stall is over.
#[test]
fn a_card_stall_is_waited_out_up_to_the_drivers_second() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = vec![0; 64 * SECTOR];
    let mut emmc = Emmc::new();
    emmc.write(&mut sim, 2, &[1; SECTOR]).unwrap();
    (sim.cost.stall_every, sim.cost.stall_ms) = (1, 900);
    let (t0, awake) = (sim.ms, sim.awake_ns);
    assert_eq!(emmc.write(&mut sim, 3, &[2; SECTOR]), Ok(()));
    assert!(sim.ms - t0 >= 900, "{} ms", sim.ms - t0);
    assert!(sim.awake_ns - awake < 10_000_000, "asleep through the stall: {} ns awake", sim.awake_ns - awake);
    sim.cost.stall_ms = 1200;
    let t0 = sim.ms;
    assert_eq!(emmc.write(&mut sim, 4, &[3; SECTOR]), Err(Error::Busy));
    assert!((1000..1100).contains(&(sim.ms - t0)), "{} ms", sim.ms - t0);
    assert!(sim.emmc.image[4 * SECTOR..5 * SECTOR].iter().all(|&b| b == 3), "the sector landed");
    sim.cost.stall_every = 0;
    emmc.retry_now();
    assert_eq!(emmc.write(&mut sim, 5, &[4; SECTOR]), Ok(()), "identified again after the stall");
    assert!(sim.emmc.cmds.iter().rev().any(|c| c.0 == 0), "from CMD0");
}
