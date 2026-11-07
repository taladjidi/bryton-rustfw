#!/usr/bin/env python3
"""Bryton's nRF52 application in a Unicorn emulator, talking the host UART protocol (docs/devloop.md, "The radio
chip's real app"). The test bench of the radio chip: the library's real radio code can drive the real app byte for
byte, with the SoftDevice replaced by stubs.

  python3 -I tools/nrf_emu.py selftest    boot, ANT event layouts, slice-size invariance, RTS hold, queue limit, hang,
                                          the commands stock never sends
  python3 -I tools/nrf_emu.py load [s]     the gap between two frames under ride load (docs/devloop.md)
  python3 -I tools/nrf_emu.py --serve      the UART as a byte pipe (src/sim/nrf_emu_tests.rs)

NOTHING of the binary is in this repository: it is read at run time from NRF_BIN (default
~/Documents/bryton/firmware/R039/nordic.bin). Its 16-byte header (u16 1, u16 init packet length, ...) gives the
app's offset; the app is loaded at 0x2D000, the S332 v5 application origin (nRF5 SDK 14.2 connectivity example
linker file). Anything specific to the binary is NOT here: an optional private JSON, named only by the environment
variable NRF_PRIVATE ({"error_handler": N}), is read when present; without it the generic detector works alone. The
ANT licence key the app passes to sd_softdevice_enable is never read, printed or stored (the stub ignores that
argument), and no message of this tool prints a register or a memory dump (an unknown SVC names its number and pc).

Why Unicorn and not Renode: Renode's nRF52840 platform models UARTE with EasyDMA, but the SoftDevice's SVCs would
need Python hooks on its SVC path and an event injector through SWI2, and the harness must pace the UART byte by byte
against a host that holds RTS and read the instruction count between two frames. Unicorn gives exactly that with a
few hundred lines: instruction hooks, MMIO hooks, and our own NVIC (Unicorn's M-class core has none).

Sources (cited per block below):
- nRF52832 Product Specification v1.4, chapter UARTE: registers, events, tasks, shorts, EasyDMA. RXD.PTR / MAXCNT and
  TXD.PTR / MAXCNT are taken at STARTRX / STARTTX (the transfer works on its own copy); RXD.AMOUNT / TXD.AMOUNT are
  updated when the transfer ends (ENDRX / ENDTX, or STOP). Not modelled: a MAXCNT of 0, UART errors, the CTS_STARTRX
  short.
- ARMv7-M Architecture Reference Manual: exception entry (B1.5.6, the 8-word frame, 8-byte stack alignment with
  xPSR bit 9 recording the padding), exception return (B1.5.8, EXC_RETURN 0xFFFFFFF9 thread mode on MSP, 0xFFFFFFF1
  handler mode), IPSR, priority (B1.5.4: lower number first, preemption only by a strictly higher priority, BASEPRI
  masks priorities numerically at or above its value, PRIMASK masks all configurable ones). nRF52 implements 3
  priority bits (the top three of each IPR byte). Cortex-M4 generic user guide 4.2 for the NVIC registers.
- SoftDevice SVC numbers: nrf_sdm.h (SDM_SVC_BASE 0x10, SD_SOFTDEVICE_ENABLE is the base), nrf_soc.h (SOC_SVC_BASE
  0x20, SOC_SVC_BASE_NOT_AVAILABLE 0x2C; SD_APP_EVT_WAIT is NOT_AVAILABLE + 21 and SD_EVT_GET + 31 in the S132 6.1.1
  header, 0x41 and 0x4B; S332 v5 predates SD_POWER_GPREGRET_SET / CLR / GET (3 entries), hence 0x3E and 0x48: the
  v5 numbers are inferred from that, the v5 header was not at hand), ble.h (BLE_SVC_BASE 0x60: ENABLE +0, EVT_GET +1,
  VERSION_GET +5, CFG_SET +9), ant_interface.h (ANT_SVC_BASE 0xC0, sd_ant_event_get 0xC1; the other ANT numbers are
  the ones the app is seen to call, no header was at hand). Signatures: SDK 14.2 components/softdevice/common
  nrf_sdh_ant.c (sd_ant_event_get), nrf_sdh_ble.c (sd_ble_evt_get), nrf_sdh_soc.c (sd_evt_get).
- SoftDevice information structure: nrf_sdm.h (SOFTDEVICE_INFO_STRUCT_OFFSET 0x2000 above the MBR, whose size is
  MBR_SIZE 0x1000 in nrf_mbr.h): size byte at +0x00, SoftDevice size at +0x08, FWID at +0x0C, ID at +0x10, version
  at +0x14.
- The BLE calls of the firmware's phone link setup, advertising and scan (src/drivers/ble.rs numbers, established there
  from the stock image): pc-ble-driver sd_api_v5 (Nordic; include/sd_api_v5/ble_gap.h, ble_gatts.h, ble.h, fetched
  2026-11-01). The numbers are the order of each SVC enum from its base (BLE_GAP_SVC_BASE 0x6C: ADV_DATA_SET 0x72,
  ADV_START 0x73, ADV_STOP 0x74, APPEARANCE_SET 0x78, PPCP_SET 0x7A, DEVICE_NAME_SET 0x7C, SCAN_START 0x8A, SCAN_STOP
  0x8B; BLE_GATTS_SVC_BASE 0xA0: SERVICE_ADD, CHARACTERISTIC_ADD 0xA2, DESCRIPTOR_ADD 0xA3, VALUE_SET 0xA4;
  BLE_UUID_VS_ADD 0x62), the arguments from the same headers without the host library's leading adapter argument: r0..r3
  in order. Outputs: sd_ble_gatts_service_add writes a u16 handle through its third argument,
  characteristic_add a ble_gatts_char_handles_t {u16 value, user_desc, cccd, sccd} through its fourth (cccd only when
  the characteristic metadata's first byte, the properties, has notify 0x10 or indicate 0x20), descriptor_add a u16 handle
  through its third, uuid_vs_add a u8 type through its second. The first handle, 0x000C, is inferred (the S132
  attribute table starts with the GAP and GATT services; the number is not in the headers).
Time: one instruction is one cycle at 64 MHz (a lower bound: Cortex-M4 averages a little above 1), a stub SVC costs
nothing (the real SoftDevice costs microseconds), the UART moves a byte every 10 us (1 Mbaud, 8N1).

Behaviour facts of the app (source: Bryton's nRF app, reversed): the event queue holds 96 slots (96 events in one
SoftDevice pass are fine, 97 hang it); a command opcode the app does not know hangs it; an ANT event code outside
ant_parameters.h's list hangs it. The inter-frame gap is measured in this emulator (selftest prints it).

Hang detector: the app is dead when it reaches a `B .` exception handler of its vector table, the private error
handler address if given, or runs more than MASKED_LIMIT instructions in a row with the UART interrupt unable to be
taken (NVIC disabled, PRIMASK, or BASEPRI at or above its priority). Sleeping executes no instruction, so a core
waiting for an interrupt never counts. MASKED_LIMIT is 10x the longest masked stretch measured over a healthy start
plus 10 s of traffic (numbers in docs/devloop.md).
"""
import collections
import heapq
import json
import os
import struct
import sys
from pathlib import Path

NRF_BIN = os.environ.get("NRF_BIN", str(Path.home() / "Documents/bryton/firmware/R039/nordic.bin"))
PRIVATE = os.environ.get("NRF_PRIVATE")  # optional, no default
APP_BASE = 0x2D000
MBR_SIZE = 0x1000
RAM, RAM_SIZE = 0x20000000, 0x10000
CPU_MHZ = 64

# SoftDevice SVC numbers (see the module doc for the sources)
SVC_SD_ENABLE, SVC_APP_EVT_WAIT, SVC_EVT_GET = 0x10, 0x3E, 0x48
SVC_BLE_ENABLE, SVC_BLE_EVT_GET, SVC_BLE_VERSION_GET, SVC_BLE_CFG_SET = 0x60, 0x61, 0x65, 0x69
SVC_ANT_EVENT_GET = 0xC1
# The ANT commands seen during the app's start and the Rust test's nrf::start / ant_start / ant_open (logged by running
# with every 0xC0+ number accepted): accepted with success and no output written, which is right for the setters
# (channel assign / id / period / frequency / open, network key, enable: inputs only). Any other ANT number stops.
ANT_NO_OUTPUT = {0xC2, 0xC4, 0xCC, 0xCD, 0xD1, 0xD3, 0xDA, 0xFA}
# SoftDevice calls of the Never commands that main's stubs (BLE_NO_OUTPUT) do not cover: connect_cancel and
# hv_confirm. The table's opcodes are the SVC numbers the app issues (BLE_SVC_BASE 0x60 + the offset in ble.h,
# ble_gap.h, ble_gattc.h of S132 v5), inferred from the v5 enumeration order. Only result 0 is stubbed.
NEVER_SVC = {0x8D, 0x9D}
# The six commands stock never sends, as rustfw's builders encode them (chip_table_tests.rs, NEVER_BYTES: the same
# hex, checked there against the builders' output).
NEVER_CMDS = ["0074", "0078c403", "008d", "009d00000d00", "00a3020001010829010121000200000200020001000101",
              "00a4ffff0300010000030001070707"]
# BLE calls with no output, see the module doc: adv_data_set, adv_start, adv_stop, appearance_set, ppcp_set,
# device_name_set, scan_start, scan_stop, gatts value_set (it writes back the length it was given: left as is)
BLE_NO_OUTPUT = {0x72, 0x73, 0x74, 0x78, 0x7A, 0x7C, 0x8A, 0x8B, 0xA4}
# The fast link's session calls, also inputs only (S132 v5 SVC enums from their bases, pc-ble-driver sd_api_v5 ble_gap.h,
# ble_gatts.h): conn_param_update 0x75, phy_update 0x8F, data_length_update 0x90 (its limitation output is NULL for us),
# gatts_hvx 0xA6 (the length is in/out and left as given), sys_attr_set 0xA9, exchange_mtu_reply 0xAD.
BLE_SESSION = {0x75, 0x8F, 0x90, 0xA6, 0xA9, 0xAD}
SVC_UUID_VS_ADD, SVC_SERVICE_ADD, SVC_CHAR_ADD, SVC_DESC_ADD = 0x62, 0xA0, 0xA2, 0xA3
EVENT_RX = 0x80  # ant_parameters.h: the broadcast data event
NOT_FOUND = 5  # NRF_ERROR_NOT_FOUND

# UARTE register offsets (nRF52832 PS, UARTE) and interrupt bits
U_STARTRX, U_STOPRX, U_STARTTX, U_STOPTX = 0x000, 0x004, 0x008, 0x00C
EV_CTS, EV_NCTS, EV_RXDRDY, EV_ENDRX, EV_TXDRDY, EV_ENDTX = 0x100, 0x104, 0x108, 0x110, 0x11C, 0x120
EV_RXTO, EV_RXSTARTED, EV_TXSTARTED, EV_TXSTOPPED = 0x144, 0x14C, 0x150, 0x158
EV_BIT = {EV_CTS: 0, EV_NCTS: 1, EV_RXDRDY: 2, EV_ENDRX: 4, EV_TXDRDY: 7, EV_ENDTX: 8, 0x124: 9, EV_RXTO: 17,
          EV_RXSTARTED: 19, EV_TXSTARTED: 20, EV_TXSTOPPED: 22}
U_SHORTS, U_INTEN, U_INTENSET, U_INTENCLR = 0x200, 0x300, 0x304, 0x308
U_RXPTR, U_RXMAX, U_RXAMT, U_TXPTR, U_TXMAX, U_TXAMT = 0x534, 0x538, 0x53C, 0x544, 0x548, 0x54C
UARTE0, UARTE_IRQ, SWI2_IRQ = 0x40002000, 2, 22
NVIC = {0xE000E100: "iser", 0xE000E180: "icer", 0xE000E200: "ispr", 0xE000E280: "icpr"}
IPR = 0xE000E400  # one priority byte per interrupt
BYTE_US = 10
MASKED_LIMIT = 2720  # instructions: 10 x the longest masked stretch measured (272, module doc)
EXC_THREAD, EXC_HANDLER = 0xFFFFFFF9, 0xFFFFFFF1
IT_BITS = 0x0600FC00  # xPSR's IT state: a handler starts without it


def ant_message(channel, event, data):
    """The ANT_MESSAGE the SoftDevice fills for an event (MESG_BUFFER_SIZE 41 bytes, SDK 14.2 nrf_sdh_ant.h ant_evt_t:
    {ANT_MESSAGE; channel; event}): data events (EVENT_RX 0x80) carry the received message [size 9, id 0x4E broadcast,
    channel, 8 bytes]; the others carry the channel event message [size 3, id 0x40, channel, 0x01, event code]
    (ANT Message Protocol 9.5.6.1). Both inferred from those documents, not seen on a chip."""
    if event == EVENT_RX:
        m = bytes([9, 0x4E, channel]) + bytes(data).ljust(8, b"\0")[:8]
    else:
        m = bytes([3, 0x40, channel, 1, event])
    return m.ljust(41, b"\0")


class Hang(Exception):
    pass


class Stop(Exception):
    """Emulation stopped for a reason the caller should see (unknown SVC, fault)."""


def have():
    """Why the emulator cannot run, or None."""
    try:
        import unicorn  # noqa: F401
    except ImportError:
        return "python unicorn missing"
    return None if Path(NRF_BIN).is_file() else f"{NRF_BIN} missing"


class Emu:
    def __init__(self, path=NRF_BIN):
        from unicorn import UC_ARCH_ARM, UC_HOOK_CODE, UC_HOOK_INTR, UC_HOOK_MEM_READ, UC_HOOK_MEM_UNMAPPED
        from unicorn import UC_HOOK_MEM_WRITE, UC_MODE_MCLASS, UC_MODE_THUMB, Uc
        d = Path(path).read_bytes()
        init_len = struct.unpack_from("<H", d, 2)[0]
        app = d[16 + init_len:]
        self.uc = uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
        uc.mem_map(0, 0x80000)  # flash: the MBR / SoftDevice region (stubs) and the app
        uc.mem_write(APP_BASE, app)
        info = MBR_SIZE + 0x2000  # SoftDevice information structure (module doc): size, SD size, FWID, ID, version
        uc.mem_write(info, bytes([0x2C]))
        uc.mem_write(info + 0x08, struct.pack("<I", APP_BASE))
        uc.mem_write(info + 0x14, struct.pack("<I", 5_000_000))  # SD_VERSION 5.0.0
        uc.mem_map(RAM, RAM_SIZE)
        for base, size in ((0x40000000, 0x100000), (0x50000000, 0x1000), (0xE0000000, 0x100000),
                           (0x10000000, 0x2000), (0xF0000000, 0x1000),):
            uc.mem_map(base, size)
        sp, rst = struct.unpack_from("<II", app, 0)
        self.reg_set("sp", sp)
        self.pc = rst | 1
        self.t_us = 0.0  # virtual time at the start of the running slice
        self.slice_i0 = 0
        self.instr = 0
        self.ev = []  # heap (t, seq, fn)
        self.seq = 0
        self.regs = collections.defaultdict(int)  # UARTE registers
        self.inten = 0
        self.nvic = collections.defaultdict(int)
        self.active = []  # exceptions being handled, outermost first: (irq, priority)
        self.started = False  # set when the UART interrupt has been enabled once
        self.stop_at = 1 << 62  # instruction count at which the next scheduled event is due
        self.returned = False  # pc is already set: do not read it back
        self.failed = None  # a Stop raised inside a hook, re-raised by run()
        self.sleep = False
        self._hold = False  # host holds RTS: the chip must not transmit
        self.fifo_limit = 0  # host RX FIFO depth: RTS drops while `out` holds this many unread bytes (0: unbounded)
        self.in_q = collections.deque()  # bytes from the host, delivered as the chip accepts them
        self.fifo = collections.deque()
        self.rx_on = self.tx_on = self.rx_armed = False
        self.rx_ptr = self.rx_max = self.rx_n = self.tx_ptr = self.tx_max = self.tx_n = 0  # the transfer's own copies
        self.out = bytearray()  # raw bytes from the chip
        self.fr_hdr, self.fr_left, self.marks = b"", None, []
        self.tx_log = []  # (kind, t_us, instr): STARTTX / ENDTX
        self.ble_evts = collections.deque()
        self.ant_evts = collections.deque()
        self.svc_log = []
        self.calls = []  # decoded BLE calls (describe), for the host test: one text line each
        self.next_handle = 0x0C  # the first attribute handle of the application's GATT table (inferred)
        self.vs_uuids = 0
        try:
            self.err_pc = json.loads(Path(PRIVATE).read_text()).get("error_handler") if PRIVATE else None
        except (OSError, ValueError):
            self.err_pc = None
        # `B .` handlers in the vector table (Cortex-M4 exception entries 2..15): reaching one is a hang, by address
        self.dead = set()
        for i in range(2, 16):
            a = struct.unpack_from("<I", app, 4 * i)[0] & ~1
            if a >= APP_BASE and struct.unpack_from("<H", app, a - APP_BASE)[0] == 0xE7FE:
                self.dead.add(a)
        self.masked = 0  # instructions in a row with the UART interrupt unable to be taken
        self.masked_max = 0
        self.hung = None  # (address, why) once the app is known to be dead
        uc.hook_add(UC_HOOK_INTR, self._intr)
        uc.hook_add(UC_HOOK_CODE, self._code)
        uc.hook_add(UC_HOOK_MEM_READ, self._mread, None, 0xE000E000, 0xE000EFFF)
        uc.hook_add(UC_HOOK_MEM_READ, self._mread, None, 0x40000000, 0x4FFFFFFF)
        uc.hook_add(UC_HOOK_MEM_WRITE, self._mwrite, None, 0x40000000, 0x4FFFFFFF)
        uc.hook_add(UC_HOOK_MEM_WRITE, self._mwrite, None, 0xE000E000, 0xE000EFFF)
        uc.hook_add(UC_HOOK_MEM_UNMAPPED, self._unmapped)

    # ---- registers
    def reg_set(self, name, v):
        from unicorn import arm_const as c
        self.uc.reg_write(getattr(c, "UC_ARM_REG_" + name.upper()), v & 0xFFFFFFFF)

    def reg(self, name):
        from unicorn import arm_const as c
        return self.uc.reg_read(getattr(c, "UC_ARM_REG_" + name.upper()))

    def rd32(self, a):
        return struct.unpack("<I", self.uc.mem_read(a, 4))[0]

    def wr32(self, a, v):
        self.uc.mem_write(a, struct.pack("<I", v & 0xFFFFFFFF))

    def _fail(self, why):
        """A Stop from inside a hook: remembered, the run ends, run() raises it."""
        self.failed = self.failed or Stop(why)
        self.uc.emu_stop()

    # ---- scheduler
    def now(self):
        """Virtual time inside a slice: the instructions run since the slice began count already."""
        return self.t_us + (self.instr - self.slice_i0) / CPU_MHZ

    def at(self, dt_us, fn):
        self.seq += 1
        heapq.heappush(self.ev, (self.now() + dt_us, self.seq, fn))
        self.stop_at = min(self.stop_at, self.instr + int(dt_us * CPU_MHZ) + 1)  # end the slice when it is due

    def _run_events(self):
        while self.ev and self.ev[0][0] <= self.t_us:
            heapq.heappop(self.ev)[2]()

    # ---- RTS / CTS (UARTE: CTS and NCTS events follow the CTS pin, our host's RTS)
    @property
    def hold(self):
        return self._hold

    @hold.setter
    def hold(self, v):
        v = bool(v)
        if v != self._hold:
            self._hold = v
            self.event(EV_NCTS if v else EV_CTS)

    # ---- hooks
    def _unmapped(self, uc, acc, addr, size, val, ud):
        self._fail(f"fault: unmapped access {addr:#x} at pc {self.reg('pc'):#x}")
        return False

    def _uart_blocked(self):
        """True when the UART interrupt cannot be taken now: NVIC disabled, PRIMASK, or BASEPRI at or above it."""
        if not (self.nvic["iser"] >> UARTE_IRQ) & 1 or self.reg("primask") & 1:
            return True
        base = self.reg("basepri")
        return bool(base) and self._prio(UARTE_IRQ) >= base

    def _code(self, uc, addr, size, ud):
        self.instr += 1
        if self.instr >= self.stop_at:
            self.stop_at = 1 << 62
            self.instr -= 1  # this instruction runs after the restart, from its own address
            self.pc = addr | 1
            self.returned = True
            self.uc.emu_stop()
            return
        if self.started and self.hung is None:
            if self._uart_blocked():
                self.masked += 1
                self.masked_max = max(self.masked_max, self.masked)
                if self.masked > MASKED_LIMIT:
                    self.hung = (addr, f"UART interrupt unable to be taken for {MASKED_LIMIT} instructions")
            else:
                self.masked = 0
            if addr == self.err_pc:
                self.hung = (addr, "reached the error handler")
            elif addr in self.dead:
                self.hung = (addr, "reached a `B .` exception handler")
            if self.hung:
                self.uc.emu_stop()

    def _intr(self, uc, no, ud):
        if no == 8:  # EXCP_EXCEPTION_EXIT: a branch to an EXC_RETURN value (pc holds it, Thumb bit dropped)
            self._exc_return(self.reg("pc") | 1)
        elif no == 2:  # SVC
            self.svc(self.uc.mem_read(self.reg("pc") - 2, 2)[0])
        else:
            self._fail(f"fault: exception {no} at pc {self.reg('pc'):#x}")

    def _mread(self, uc, acc, addr, size, val, ud):
        if addr in NVIC:
            self.wr32(addr, self.nvic["iser"] if NVIC[addr] in ("iser", "icer") else self.nvic["ispr"])
        elif UARTE0 <= addr < UARTE0 + 0x1000:
            off = addr - UARTE0
            self.wr32(addr, self.inten if off in (U_INTEN, U_INTENSET, U_INTENCLR) else self.regs[off])

    def _mwrite(self, uc, acc, addr, size, val, ud):
        if addr in NVIC:
            k = NVIC[addr]
            n = self.nvic
            if k == "iser":
                n["iser"] |= val
                self.started = self.started or bool((val >> UARTE_IRQ) & 1)
            elif k == "icer":
                n["iser"] &= ~val
            elif k == "ispr":
                n["ispr"] |= val
            else:
                n["ispr"] &= ~val
        elif UARTE0 <= addr < UARTE0 + 0x1000:
            self.uarte_write(addr - UARTE0, val)

    # ---- UARTE0 (nRF52832 PS chapter UARTE; EasyDMA, shorts ENDRX_STARTRX bit 5 / ENDRX_STOPRX bit 6)
    def uarte_write(self, off, v):
        r = self.regs
        if off == U_STARTRX and v:
            self.start_rx()
        elif off == U_STOPRX and v:
            if self.rx_on:
                self.end_rx()
            self.event(EV_RXTO)
        elif off == U_STARTTX and v:
            self.start_tx()
        elif off == U_STOPTX and v:
            if self.tx_on:
                self.end_tx()
            self.event(EV_TXSTOPPED)
        elif off == U_INTEN:
            self.inten = v
        elif off == U_INTENSET:
            self.inten |= v
        elif off == U_INTENCLR:
            self.inten &= ~v
        else:
            r[off] = v  # an event cleared (0), a pointer, the shorts: plain registers

    def event(self, off):
        self.regs[off] = 1

    def uart_irq(self):
        return any(self.regs[o] and (self.inten >> b) & 1 for o, b in EV_BIT.items())

    def start_rx(self):
        r = self.regs
        self.rx_ptr, self.rx_max, self.rx_n, self.rx_on = r[U_RXPTR], r[U_RXMAX], 0, True  # latched here, AMOUNT kept
        self.event(EV_RXSTARTED)
        self.arm_rx()

    def end_rx(self):
        self.rx_on = False
        self.regs[U_RXAMT] = self.rx_n
        self.event(EV_ENDRX)

    def start_tx(self):
        if self.tx_on:
            return
        r = self.regs
        self.tx_ptr, self.tx_max, self.tx_n, self.tx_on = r[U_TXPTR], r[U_TXMAX], 0, True
        self.event(EV_TXSTARTED)
        self.tx_log.append(("start", self.now(), self.instr))
        if self.fr_left is None and not self.fr_hdr:
            self.marks.append(("fcall", self.instr))  # the app starts the next frame's header
        self.at(BYTE_US, self.tx_pump)

    def end_tx(self):
        self.tx_on = False
        self.regs[U_TXAMT] = self.tx_n
        self.event(EV_ENDTX)
        self.tx_log.append(("end", self.now(), self.instr))

    def rx_ready(self):
        """Chip's RTS: True when it accepts a byte (a receive armed, or room in the 4-byte FIFO)."""
        return self.rx_on or len(self.fifo) < 4

    def arm_rx(self):
        if not self.rx_armed:  # one pump at a time: parallel ones would deliver bytes faster than the wire
            self.rx_armed = True
            self.at(0, self.rx_pump)

    def rx_pump(self):
        self.rx_armed = False
        while self.fifo and self.rx_on:
            self.rx_byte(self.fifo.popleft())
        if self.in_q and self.rx_ready():
            b = self.in_q.popleft()
            (self.rx_byte if self.rx_on else self.fifo.append)(b)
        if self.in_q:
            self.rx_armed = True
            self.at(BYTE_US, self.rx_pump)

    def rx_byte(self, b):
        r = self.regs
        self.uc.mem_write(self.rx_ptr + self.rx_n, bytes([b]))
        self.rx_n += 1
        self.event(EV_RXDRDY)
        if self.rx_n >= self.rx_max:
            self.end_rx()
            if r[U_SHORTS] & 0x20:
                self.start_rx()  # the next transfer starts from the pointer in the registers now; AMOUNT stays
            elif r[U_SHORTS] & 0x40:
                self.event(EV_RXTO)

    def _mark(self, b):
        """Frame boundaries in instructions: ('fstart'|'fend', instr)."""
        if self.fr_left is None:
            self.fr_hdr += bytes(b)
            if len(self.fr_hdr) == 1:
                self.marks.append(("fstart", self.instr))
            if len(self.fr_hdr) == 2:
                self.fr_left = struct.unpack("<H", self.fr_hdr)[0]
                self.fr_hdr = b""
        else:
            self.fr_left -= 1
        if self.fr_left == 0:
            self.fr_left = None
            self.marks.append(("fend", self.instr))

    def tx_pump(self):
        if not self.tx_on:
            return
        if self.hold or (self.fifo_limit and len(self.out) >= self.fifo_limit):  # CTS high: wait, no time-out
            self.at(BYTE_US, self.tx_pump)
            return
        b = self.uc.mem_read(self.tx_ptr + self.tx_n, 1)
        self.out += b
        self._mark(b)
        self.tx_n += 1
        self.event(EV_TXDRDY)
        if self.tx_n >= self.tx_max:
            self.end_tx()
        else:
            self.at(BYTE_US, self.tx_pump)

    # ---- SoftDevice stubs
    def alloc_handles(self, k):
        """The first of k fresh attribute handles."""
        h, self.next_handle = self.next_handle, self.next_handle + k
        return h

    def svc(self, n):
        a = [self.reg(x) for x in ("r0", "r1", "r2", "r3")]
        self.svc_log.append(n)
        if n not in (SVC_APP_EVT_WAIT, SVC_EVT_GET, SVC_BLE_EVT_GET, SVC_ANT_EVENT_GET):
            self.calls.append(self.describe(n, a))
        ret = 0
        if n == SVC_SD_ENABLE:
            pass  # sd_softdevice_enable(clock cfg, fault handler): no output; the ANT licence key is never looked at
        elif n == SVC_APP_EVT_WAIT:  # sd_app_evt_wait()
            self.sleep = True
            self.pc = self.reg("pc") | 1  # the slice ends here, whatever its size: results must not depend on it
            self.returned = True
            self.uc.emu_stop()
        elif n in (SVC_BLE_ENABLE, SVC_BLE_CFG_SET):
            pass  # sd_ble_enable(&ram_base), sd_ble_cfg_set(id, cfg, ram_base): the RAM base is left as given (enough)
        elif n == SVC_BLE_VERSION_GET:  # sd_ble_version_get(ble_version_t *): {u8 version, u16 company, u16 subversion}
            self.uc.mem_write(a[0], struct.pack("<BxHH", 9, 0x59, 0x1234))
        elif n == SVC_BLE_EVT_GET:  # sd_ble_evt_get(u8 *dest, u16 *len)
            if self.ble_evts:
                e = self.ble_evts.popleft()
                self.uc.mem_write(a[0], e)
                self.uc.mem_write(a[1], struct.pack("<H", len(e)))
            else:
                ret = NOT_FOUND
        elif n == SVC_EVT_GET:  # sd_evt_get(u32 *id): no SoC events
            ret = NOT_FOUND
        elif n == SVC_ANT_EVENT_GET:  # sd_ant_event_get(u8 *channel, u8 *event, u8 *msg buffer of 41 bytes)
            if self.ant_evts:
                ch, ev, buf = self.ant_evts.popleft()
                self.uc.mem_write(a[0], bytes([ch]))
                self.uc.mem_write(a[1], bytes([ev]))
                self.uc.mem_write(a[2], buf)
            else:
                ret = NOT_FOUND
        elif n in BLE_NO_OUTPUT or n in BLE_SESSION:
            pass
        elif n == SVC_UUID_VS_ADD:  # sd_ble_uuid_vs_add(ble_uuid128_t const *, uint8_t *type): types start at 2
            self.vs_uuids += 1
            self.uc.mem_write(a[1], bytes([1 + self.vs_uuids]))
        elif n == SVC_SERVICE_ADD:  # sd_ble_gatts_service_add(type, ble_uuid_t const *, uint16_t *handle)
            self.uc.mem_write(a[2], struct.pack("<H", self.alloc_handles(1)))
        elif n == SVC_CHAR_ADD:  # sd_ble_gatts_characteristic_add(service, char_md *, attr *, ble_gatts_char_handles_t *)
            notify = self.uc.mem_read(a[1], 1)[0] & 0x30
            value = self.alloc_handles(2)  # the declaration, then the value
            self.uc.mem_write(a[3], struct.pack("<HHHH", value + 1, 0, self.alloc_handles(1) if notify else 0, 0))
        elif n == SVC_DESC_ADD:  # sd_ble_gatts_descriptor_add(char_handle, attr *, uint16_t *handle)
            self.uc.mem_write(a[2], struct.pack("<H", self.alloc_handles(1)))
        elif n in ANT_NO_OUTPUT or n in NEVER_SVC:
            pass
        else:
            self._fail(f"unknown SVC {n:#x} at pc {self.reg('pc'):#x}")
            return
        self.reg_set("r0", ret)

    def mem(self, a, n):
        """n bytes of RAM at a, or None when a is NULL or outside the RAM."""
        return bytes(self.uc.mem_read(a, n)) if RAM <= a and a + n <= RAM + RAM_SIZE else None

    def describe(self, n, a):
        """One line for a BLE call, decoded as the S132 v5 headers lay the arguments out (module doc). Bond data is
        never printed (sys_attr_set shows its length only)."""
        def u16s(p, k):
            m = self.mem(p, 2 * k)
            return "null" if m is None else ",".join(str(x) for x in struct.unpack(f"<{k}H", m))
        if n == SVC_SD_ENABLE:
            return "softdevice_enable"
        if n == SVC_BLE_VERSION_GET:
            return "version_get"
        if n == SVC_BLE_ENABLE:
            m = self.mem(a[0], 4)
            return f"enable ram_base={struct.unpack('<I', m)[0]:#x}" if m else "enable"
        if n == SVC_BLE_CFG_SET:  # ble_cfg_t: a connection configuration is {u8 tag; union}, the union at +2
            c = self.mem(a[1], 6) or bytes(6)
            what = {0x20: f"conn_count={c[2]} event_length={c[4]}", 0x22: f"hvn_tx_queue_size={c[2]}",
                    0x23: f"att_mtu={c[2] | c[3] << 8}", 0x40: f"periph={c[0]} central={c[1]} central_sec={c[2]}"}
            return f"cfg_set id={a[0]:#x} " + (f"tag={c[0]} " if 0x20 <= a[0] < 0x40 else "") + what.get(a[0], "")
        if n == 0x75:
            return f"conn_param_update conn={a[0]} params={u16s(a[1], 4)}"
        if n == 0x8F:
            m = self.mem(a[1], 2)
            return f"phy_update conn={a[0]} phys={'null' if m is None else f'{m[0]},{m[1]}'}"
        if n == 0x90:
            return f"data_length_update conn={a[0]} params={u16s(a[1], 4)} limitation={'null' if not a[2] else 'set'}"
        if n == 0xA6:  # ble_gatts_hvx_params_t {u16 handle; u8 type; u16 offset; u16 *p_len; u8 const *p_data}
            h = self.mem(a[1], 16)
            if h is None:
                return f"hvx conn={a[0]} params=null"
            handle, ty, off, p_len, p_data = struct.unpack("<HBxH2xII", h)
            ln = self.mem(p_len, 2)
            ln = struct.unpack("<H", ln)[0] if ln else -1
            d = self.mem(p_data, ln) if ln > 0 else b""
            return (f"hvx conn={a[0]} handle={handle:#x} type={ty} offset={off} len={ln} "
                    f"data={'null' if d is None else d.hex()}")
        if n == 0xA9:
            return f"sys_attr_set conn={a[0]} data={'null' if not a[1] else 'set'} len={a[2] & 0xFFFF} flags={a[3]:#x}"
        if n == 0xAD:
            return f"exchange_mtu_reply conn={a[0]} mtu={a[1] & 0xFFFF}"
        if n == SVC_UUID_VS_ADD:
            return "uuid_vs_add"
        return f"svc {n:#x}"

    # ---- NVIC and exceptions (ARMv7-M; Unicorn's M core has no NVIC, so entry and return are done here)
    def _prio(self, irq):
        return self.uc.mem_read(IPR + irq, 1)[0] & 0xE0

    def _exc_enter(self, irq):
        sp = self.reg("sp")
        pad = sp & 4  # the frame is 8-byte aligned; xPSR bit 9 records the padding
        sp = (sp - 32) & ~7
        x = self.reg("xpsr")
        frame = [self.reg(r) for r in ("r0", "r1", "r2", "r3", "r12", "lr")] + [self.pc & ~1, x | (pad << 7)]
        self.uc.mem_write(sp, struct.pack("<8I", *frame))
        self.reg_set("sp", sp)
        self.reg_set("lr", EXC_HANDLER if self.active else EXC_THREAD)
        self.reg_set("xpsr", (x & ~(0x1FF | IT_BITS)) | (irq + 16))  # IPSR
        self.pc = self.rd32(APP_BASE + 4 * (16 + irq)) | 1
        self.active.append((irq, self._prio(irq)))
        self.returned = True

    def _exc_return(self, excret):
        if (excret & 0xF) == 0xD or not self.active or (excret & 8) != (0 if len(self.active) > 1 else 8):
            self._fail(f"fault: bad exception return at pc {self.reg('pc'):#x}")
            return
        self.active.pop()
        sp = self.reg("sp")
        f = struct.unpack("<8I", self.uc.mem_read(sp, 32))
        for x, v in zip(("r0", "r1", "r2", "r3", "r12", "lr"), f):
            self.reg_set(x, v)
        self.reg_set("sp", sp + 32 + (4 if f[7] & 0x200 else 0))
        self.reg_set("xpsr", f[7] & ~0x200)  # the thread's flags and IT state, and the outer IPSR
        self.pc = f[6] | 1
        self.returned = True
        self.uc.emu_stop()

    def _take_irq(self):
        """The interrupt to enter now, or None: pending and enabled, not masked by PRIMASK / BASEPRI, and of strictly
        higher priority (lower number) than everything being handled; ties go to the lower interrupt number."""
        n = self.nvic
        if self.uart_irq():
            n["ispr"] |= 1 << UARTE_IRQ  # the peripheral line is level: still high after the ISR, it pends again
        m = n["ispr"] & n["iser"]
        if not m or self.reg("primask") & 1:
            return None
        base = self.reg("basepri")
        cur = min((p for _, p in self.active), default=256)
        best = None
        for irq in range(32):
            if (m >> irq) & 1:
                p = self._prio(irq)
                if p < cur and not (base and p >= base) and (best is None or p < best[0]):
                    best = (p, irq)
        return best and best[1]

    # ---- run
    def run(self, us, until=None, slice_n=2000):
        """Runs `us` of virtual time (or until `until()`); returns when done. Raises Hang or Stop."""
        from unicorn import UcError
        end = self.t_us + us
        while self.t_us < end and not (until and until()):
            self._run_events()
            irq = self._take_irq()
            if irq is not None:
                self.nvic["ispr"] &= ~(1 << irq)
                self.sleep = False
                self._exc_enter(irq)
            if self.hung:
                raise Hang(f"{self.hung[1]} at {self.hung[0]:#x}")
            if self.sleep and self.started and self._uart_blocked() and not self.active:
                # the chip idles with the UART interrupt masked: nothing of ours can ever wake it
                raise Hang(f"asleep at {self.pc:#x} with the UART interrupt unable to be taken")
            if self.sleep:  # sd_app_evt_wait: jump to the next thing that can wake the chip
                nxt = self.ev[0][0] if self.ev else end
                self.t_us = min(max(nxt, self.t_us), end)
                if not self.ev and not self._take_irq():
                    return
                continue
            # stop at the next scheduled event so an interrupt is taken on time (the gap measure needs it)
            self.stop_at = (self.instr + int((self.ev[0][0] - self.t_us) * CPU_MHZ) + 1) if self.ev else 1 << 62
            self.stop_at = min(self.stop_at, self.instr + int((end - self.t_us) * CPU_MHZ) + 1)  # never past `end`
            self.returned = False
            self.sleep = False  # only an sd_app_evt_wait inside this slice puts the core to sleep
            n0 = self.slice_i0 = self.instr
            try:
                self.uc.emu_start(self.pc, 0xFFFFFFF0, count=slice_n)
            except UcError as e:
                raise Stop(f"fault: {e} at pc {self.reg('pc'):#x}")
            if self.failed:
                raise self.failed
            if not self.returned:
                self.pc = self.reg("pc") | 1
            self.t_us += (self.instr - n0) / CPU_MHZ
            self.slice_i0 = self.instr
            if self.hung:
                raise Hang(f"{self.hung[1]} at {self.hung[0]:#x}")

    # ---- host side of the wire
    def send(self, payload):
        self.in_q.extend(struct.pack("<H", len(payload)) + bytes(payload))
        self.arm_rx()

    def send_raw(self, b):
        self.in_q.extend(b)
        self.arm_rx()

    def frames(self):
        """Complete frames received so far (payloads); consumed."""
        out = []
        while len(self.out) >= 2:
            n = struct.unpack_from("<H", self.out)[0]
            if len(self.out) < 2 + n:
                break
            out.append(bytes(self.out[2:2 + n]))
            del self.out[:2 + n]
        return out

    def inject_ant(self, channel, event, data=b"", queue_only=False):
        """A SoftDevice ANT event (ant_event_get) and the SWI2 interrupt that announces it."""
        self.ant_evts.append((channel, event, ant_message(channel, event, data)))
        if not queue_only:
            self.nvic["ispr"] |= 1 << SWI2_IRQ

    def inject_ble(self, raw):
        self.ble_evts.append(bytes(raw))
        self.nvic["ispr"] |= 1 << SWI2_IRQ

    def gaps(self, to="fstart"):
        """Instructions from the end of a transmitted frame to the next one's first byte on the wire ("fstart") or
        to the app starting its header DMA ("fcall": the app's own work)."""
        g, end = [], None
        for k, i in self.marks:
            if k == "fend":
                end = i
            elif k == to and end is not None:
                g.append(i - end)
                end = None
        return g


def boot_emu(**kw):
    """An Emu run to the app's idle wait."""
    e = Emu()
    e.run(50_000, until=lambda: bool(e.svc_log) and e.sleep, **kw)
    return e


def selftest():
    why = have()
    if why:
        print(f"skip: nrf_emu selftest ({why})")
        return 0
    e = boot_emu()
    assert e.sleep, f"the app did not reach its idle wait; svcs {[hex(x) for x in e.svc_log]}"
    e.send(bytes([0, 0x65, 1]))
    e.run(5_000)
    f = e.frames()
    assert f and f[0][:2] == bytes([1, 0x65]) and len(f[0]) == 2 + 4 + 5, f"version answer of {len(f)} frames"
    assert f[0][2:6] == bytes(4), "result not success"
    # injected ANT events come out as type 08 frames of 44 bytes: [08][41 message bytes][event][channel], the layout
    # rustfw's nrf.rs parses (ant_broadcast, ant_closed, ant_tx_failed, ant_tx_completed, search time-out)
    for code in (EVENT_RX, 0x01, 0x05, 0x06, 0x07):
        e.inject_ant(3, code, bytes(range(8)))
        e.run(2_000)
        fr = e.frames()
        assert len(fr) == 1 and fr[0][0] == 8 and len(fr[0]) == 44, [(p[0], len(p)) for p in fr]
        assert fr[0][42:] == bytes([code, 3]), f"event code {code:#x}: {fr[0][42:].hex()}"
        want = bytes([9, 0x4E, 3]) + bytes(range(8)) if code == EVENT_RX else bytes([3, 0x40, 3, 1, code])
        assert fr[0][1:1 + len(want)] == want
    print("ok: ANT events 0x80, 0x01, 0x05, 0x06, 0x07 keep their layout")
    # a burst: one SWI2 pass brings 8 events, so the frames follow each other from the queue
    e.marks.clear()
    for i in range(8):
        e.inject_ant(1, EVENT_RX, bytes([i]), queue_only=True)
    e.nvic["ispr"] |= 1 << SWI2_IRQ
    e.run(5_000)
    assert [p[4] for p in e.frames()] == list(range(8))
    g, w = e.gaps("fcall"), e.gaps()
    assert len(g) == 7 and len(w) == 7
    print(f"ok: 8 queued events; ENDTX to the next STARTTX {g[0]} instr = {g[0] / CPU_MHZ:.0f} us at {CPU_MHZ} MHz "
          f"(CPI 1), {w[0]} instr to the first byte on the wire ({w[0] / CPU_MHZ:.0f} us)")
    print(f"ok: longest stretch with the UART interrupt unable to be taken so far: {e.masked_max} instructions")

    # results do not depend on the slice size
    def script(n):
        x = boot_emu(slice_n=n)
        x.send(bytes([0, 0x65, 1]))
        for i in range(10):
            x.inject_ant(1, EVENT_RX, bytes([i]), queue_only=i % 2 == 0)
            x.run(1_500, slice_n=n)
        x.run(20_000, slice_n=n)
        return bytes(x.out), x.tx_log
    assert script(2000) == script(97) == script(5000), "frames or timing depend on the slice size"
    print("ok: same frames and TX times at slice sizes 97, 2000, 5000")
    # RTS held for 200 ms while 30 events arrive: nothing on the wire, no hang, every frame in order when released
    h = boot_emu()
    h.hold = True
    for i in range(30):
        h.inject_ant(1, EVENT_RX, bytes([i]))
        h.run(6_600)
    h.run(2_000)
    assert len(h.out) == 0 and any(k == "start" for k, _, _ in h.tx_log), "TX started and the held RTS stopped it"
    h.hold = False
    h.run(20_000)
    assert [p[4] for p in h.frames()] == list(range(30))
    print("ok: RTS held 200 ms, 30 events: nothing sent, no hang, 30 frames in order after the release")
    # the event queue holds 96 events (Bryton's nRF app, reversed): 96 in one pass are fine, 97 hang the app
    for n, hangs in ((96, False), (97, True)):
        q = boot_emu()
        q.hold = True
        for i in range(n):
            q.inject_ant(1, EVENT_RX, bytes([i & 255]), queue_only=True)
        q.nvic["ispr"] |= 1 << SWI2_IRQ
        try:
            q.run(5_000)
            hung = False
        except Hang:
            hung = True
        assert hung == hangs, f"{n} events in one pass: hung {hung}"
    print("ok: 96 events in one SoftDevice pass are queued, 97 hang the app")
    # an unknown opcode: the chip hangs, no response, at the same instruction whatever the slice size
    seen = set()
    for n in (97, 2000):
        e2 = boot_emu(slice_n=n)
        e2.send(bytes([0, 0x01]))
        try:
            e2.run(100_000, slice_n=n)
        except Hang as ex:
            seen.add((str(ex), e2.instr))
        else:
            raise AssertionError("an unknown opcode did not hang the app")
        assert not e2.frames()
    assert len(seen) == 1, seen
    print(f"ok: unknown opcode hangs ({seen.pop()[0]}), identical at two slice sizes")
    # the commands stock never sends: each is answered [01][op][result u32] and does not hang the app
    for hx in NEVER_CMDS:
        pl = bytes.fromhex(hx)
        n2 = boot_emu()
        n2.send(pl)
        try:
            n2.run(50_000)
        except Hang as ex:
            raise AssertionError(f"command {pl[1]:#x} hung the app: {ex}") from None
        fr = n2.frames()
        assert len(fr) == 1 and fr[0][:2] == bytes([1, pl[1]]) and len(fr[0]) >= 6, \
            f"command {pl[1]:#x}: {[p.hex() for p in fr]}"
    print(f"ok: {len(NEVER_CMDS)} commands stock never sends are answered [01][op][result], no hang")
    return 0


def hvx_raw(n):
    """One BLE GATTC HVX event of n data bytes as the SoftDevice hands it over. S132/S332 v5 ble_evt_t: {u16 evt_id 0x39
    BLE_GATTC_EVT_HVX, u16 evt_len}, gattc {conn_handle, gatt_status, error_handle}, hvx {handle, type, len, data}
    (inferred from the v5 headers). evt_len counts the 4-byte header too: the app reads a shorter one as zeros (found
    2026-11-07 by trying lengths; a lone data byte used to come out with a zero length for that reason)."""
    body = struct.pack("<HHHHBxH", 0, 0, 0, 0x0F, 1, n) + bytes(i & 255 for i in range(n))
    return struct.pack("<HH", 0x39, len(body) + 4) + body


def load(seconds=20):
    """The gap between two frames under ride load (the host drains at once): three ANT channels at about 4 Hz, a BLE
    trainer's notifications at 10 Hz, and a phone's burst of 12 notifications every 5 s. Prints the app's own work
    between the end of a frame and the next frame's header DMA, in instructions and us at CPU_MHZ."""
    e = boot_emu()
    e.marks.clear()
    us = seconds * 1_000_000
    for ch, (per, off) in enumerate(((250_000, 0), (246_000, 80_000), (252_000, 160_000))):
        for k in range(us // per):
            e.at(off + per * k, lambda ch=ch: e.inject_ant(ch, EVENT_RX, bytes(range(8))))
    for k in range(us // 100_000):
        e.at(50_000 + 100_000 * k, lambda: e.inject_ble(hvx_raw(13)))
    for k in range(us // 5_000_000):
        e.at(2_500_000 + 5_000_000 * k, lambda: [e.inject_ble(hvx_raw(20 + i)) for i in range(12)])
    e.run(us)
    for what, g in (("to the next STARTTX (the app's work)", e.gaps("fcall")), ("to the first byte on the wire", e.gaps())):
        g = sorted(g)
        q = lambda f: g[min(len(g) - 1, int(len(g) * f))]
        print(f"{len(g)} gaps, ENDTX {what}: min {g[0]} / median {q(.5)} / p99 {q(.99)} / max {g[-1]} instr "
              f"= {g[0] / CPU_MHZ:.0f} / {q(.5) / CPU_MHZ:.0f} / {q(.99) / CPU_MHZ:.0f} / {g[-1] / CPU_MHZ:.0f} us at {CPU_MHZ} MHz")
    print(f"frames out {len(e.frames())}, longest masked stretch {e.masked_max} instructions, hung {bool(e.hung)}")


def serve(inp=None, out=None):
    """The chip's UART as a byte pipe on stdin / stdout, for the Rust host test (src/sim/nrf_emu_tests.rs). The host
    sends requests, the harness answers each with [status][flags] and its payload. Status 0 ok; 1 the app hung or the
    emulation stopped (then u16 length and the reason). Flags: bit 0 the chip holds CTS (it does not take bytes: its
    4-byte FIFO is full), bit 1 the host's RTS is held (the chip's transmitter waits), bit 2 the app is dead.
    All integers little endian. Requests:
      'W' u16 n, bytes        host to chip bytes (queued; the chip takes one per 10 us of emulated time)
      'R' u16 max             chip to host bytes, at most max: answer u16 n, bytes
      'T' u32 us              advance emulated time
      'H' u8                  hold (1) or release (0) the host's RTS
      'A' ch ev n, n bytes    one ANT event now (n data bytes for ev 0x80), SWI2 pended
      'B' ch ev, 8 bytes, u32 period_us, u16 count   count ANT broadcasts, one per period
      'V' u16 n               one BLE GATTC HVX event carrying n bytes
      'E' u16 n, n bytes      one raw SoftDevice BLE event (ble_evt_t), SWI2 pended
      'C'                     how many BLE calls so far (then cleared): u16 4, u32 count
      'S'                     the decoded BLE calls so far, one per line (Emu.describe), then cleared: u16 n, text
      'F' u16 n               the host's RX FIFO holds n bytes: RTS drops while that many are unread (0: unbounded)
      'N'                     bytes of 'W' the chip has not taken yet (not on the wire into it): answer u16 2, u16 n
      'X'                     restart the app (the chip's reset), run to its idle wait
      'Q'                     quit"""
    inp, out = inp or sys.stdin.buffer, out or sys.stdout.buffer
    e = [None]

    def reply(err=None, payload=b""):
        x = e[0]
        fl = (not x.rx_ready()) | x.hold << 1 | bool(x.hung) << 2
        if err:
            out.write(bytes([1, fl]) + struct.pack("<H", len(err)) + err.encode())
        else:
            out.write(bytes([0, fl]) + payload)
        out.flush()

    e[0] = boot_emu()
    while True:
        c = inp.read(1)
        if not c or c == b"Q":
            return 0
        x, err, payload = e[0], None, b""
        if c == b"W":
            x.send_raw(inp.read(struct.unpack("<H", inp.read(2))[0]))
        elif c == b"R":
            n = min(struct.unpack("<H", inp.read(2))[0], len(x.out))
            payload = struct.pack("<H", n) + bytes(x.out[:n])
            del x.out[:n]
        elif c == b"T":
            try:
                x.run(struct.unpack("<I", inp.read(4))[0])
            except (Hang, Stop) as ex:
                err = str(ex)
        elif c == b"H":
            x.hold = bool(inp.read(1)[0])
        elif c == b"A":
            ch, ev, n = inp.read(3)
            x.inject_ant(ch, ev, inp.read(n))
        elif c == b"B":
            ch, ev = inp.read(2)
            data = inp.read(8)
            period, count = struct.unpack("<IH", inp.read(6))
            for i in range(count):
                x.at(period * (i + 1), lambda ch=ch, ev=ev, data=data: x.inject_ant(ch, ev, data))
        elif c == b"V":
            x.inject_ble(hvx_raw(struct.unpack("<H", inp.read(2))[0]))
        elif c == b"E":
            x.inject_ble(inp.read(struct.unpack("<H", inp.read(2))[0]))
        elif c == b"S":
            t = "\n".join(x.calls).encode()
            x.calls.clear()
            payload = struct.pack("<H", len(t)) + t
        elif c == b"C":
            payload = struct.pack("<HI", 4, len(x.calls))
            x.calls.clear()
        elif c == b"F":
            x.fifo_limit = struct.unpack("<H", inp.read(2))[0]
        elif c == b"N":
            payload = struct.pack("<HH", 2, len(x.in_q))
        elif c == b"X":
            e[0] = boot_emu()
        else:
            err = f"unknown request {c!r}"
        reply(err, payload)


if __name__ == "__main__":
    if sys.argv[1:] == ["selftest"]:
        sys.exit(selftest())
    if sys.argv[1:2] == ["load"]:
        load(int(sys.argv[2]) if sys.argv[2:] else 20)
        sys.exit(0)
    if sys.argv[1:] == ["--serve"]:
        sys.exit(serve())
    print(__doc__)
    sys.exit(2)
