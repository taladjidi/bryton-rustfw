#!/usr/bin/env python3
"""Bryton's nRF52 application in a Unicorn emulator, talking the host UART protocol (docs/devloop.md, "The radio
chip's real app"). The test bench of the radio chip: the library's real radio code can drive the real app byte for
byte, with the SoftDevice replaced by stubs.

  python3 -I tools/nrf_emu.py selftest    boot, ANT event layouts, slice-size invariance, RTS hold, queue limit, hang
  python3 -I tools/nrf_emu.py --serve      the UART as a byte pipe (src/sim/nrf_emu_tests.rs)

NOTHING of the binary is in this repository: it is read at run time from NRF_BIN (default
/Users/tangui/Documents/bryton/firmware/R039/nordic.bin). Its 16-byte header (u16 1, u16 init packet length, ...) gives
the app's offset; the app is loaded at 0x2D000, the S332 v5 application origin (nRF5 SDK 14.2 connectivity example
linker file). Anything specific to the binary (a breakpoint, the hang detector's help) is NOT here: an optional
private JSON (NRF_PRIVATE, default ~/Documents/bryton-S510-mods/ghidra/nordic-app-addrs.json, {"error_handler": N})
is read when present; without it the generic detector below works alone. The ANT licence key the app passes to
sd_softdevice_enable is never read, printed or stored (the stub ignores that argument).

Why Unicorn and not Renode: Renode's nRF52840 platform models UARTE with EasyDMA, but the SoftDevice's SVCs would
need Python hooks on its SVC path and an event injector through SWI2, and the harness must pace the UART byte by byte
against a host that holds RTS and read the instruction count between two frames. Unicorn gives exactly that with a
few hundred lines: instruction hooks, MMIO hooks, and our own exception entry (Unicorn's M-class core has no NVIC).

Models (all from the nRF52832 product specification v1.x, cited per block): UARTE0 with EasyDMA (chapter UARTE),
the NVIC set/clear registers (Cortex-M4 generic user guide, 4.2), POWER/CLOCK/RTC/GPIO as plain registers that read
back what was written (the app only configures them). SoftDevice SVC numbers: Nordic's public headers
(nrf_sdm.h SD_SVC_BASE 0x10, nrf_soc.h SOC_SVC_BASE 0x20, ble.h BLE_SVC_BASE 0x60, ant_interface.h ANT_SVC_BASE 0xC0);
signatures: SDK 14.2 components/softdevice/common/nrf_sdh_ant.c (sd_ant_event_get), nrf_sdh_ble.c (sd_ble_evt_get).
Time: one instruction is one cycle at 64 MHz (a lower bound: Cortex-M4 averages a little above 1), a stub SVC costs
nothing (the real SoftDevice costs microseconds), the UART moves a byte every 10 us (1 Mbaud, 8N1).
"""
import collections
import heapq
import json
import os
import struct
import sys
from pathlib import Path

NRF_BIN = os.environ.get("NRF_BIN", "/Users/tangui/Documents/bryton/firmware/R039/nordic.bin")
PRIVATE = os.environ.get("NRF_PRIVATE", str(Path.home() / "Documents/bryton-S510-mods/ghidra/nordic-app-addrs.json"))
APP_BASE = 0x2D000
RAM, RAM_SIZE = 0x20000000, 0x10000
RET = 0x7F000  # exception-return magic: a page with `b .`, caught by the code hook
CPU_MHZ = 64

# SoftDevice SVC numbers (public headers, see the module doc)
SVC_SD_ENABLE, SVC_APP_EVT_WAIT = 0x10, 0x3E
SVC_BLE_ENABLE, SVC_BLE_EVT_GET, SVC_BLE_VERSION_GET, SVC_BLE_CFG_SET = 0x60, 0x61, 0x65, 0x69
SVC_ANT_EVENT_GET = 0xC1
SVC_EVT_GET = 0x48  # inferred: called after the ANT poll with one pointer (nrf_sdh_soc.c polls sd_evt_get)
EVENT_RX = 0x80  # ant_parameters.h: the broadcast data event. Event codes outside ant_event.c's switch (0x4F, 0x2F:
# message ids, not event codes) make the encoder return NOT_SUPPORTED and the app hangs: that was the "bug".
NOT_FOUND = 5  # NRF_ERROR_NOT_FOUND

# UARTE register offsets (nRF52832 PS, UARTE) and interrupt bits
U_STARTRX, U_STOPRX, U_STARTTX, U_STOPTX = 0x000, 0x004, 0x008, 0x00C
EV_ENDRX, EV_RXDRDY, EV_ENDTX, EV_TXDRDY, EV_RXTO = 0x110, 0x108, 0x120, 0x11C, 0x144
EV_BIT = {0x100: 0, 0x104: 1, 0x108: 2, 0x110: 4, 0x11C: 7, 0x120: 8, 0x124: 9, 0x144: 17, 0x14C: 19, 0x150: 20, 0x158: 22}
U_SHORTS, U_INTENSET, U_INTENCLR = 0x200, 0x304, 0x308
U_RXPTR, U_RXMAX, U_RXAMT, U_TXPTR, U_TXMAX, U_TXAMT = 0x534, 0x538, 0x53C, 0x544, 0x548, 0x54C
UARTE0, UARTE_IRQ, SWI2_IRQ = 0x40002000, 2, 22
NVIC = {0xE000E100: "iser", 0xE000E180: "icer", 0xE000E200: "ispr", 0xE000E280: "icpr"}
BYTE_US = 10
MASKED_LIMIT = 20_000  # instructions


def ant_message(channel, event, data):
    """The ANT_MESSAGE the SoftDevice fills for an event (MESG_BUFFER_SIZE 41 bytes, SDK 14.2 nrf_sdh_ant.h ant_evt_t:
    {ANT_MESSAGE; channel; event}): data events (EVENT_RX 0x80) carry the received message [size 9, id 0x4E broadcast,
    channel, 8 bytes]; the others carry the channel event message [size 3, id 0x40, channel, 0x01, event code]
    (ANT Message Protocol 9.5.6.1). Both inferred from those documents, not seen on a chip."""
    m = bytes([9, 0x4E, channel]) + bytes(data).ljust(8, b"\0")[:8] if event == EVENT_RX else bytes([3, 0x40, channel, 1, event])
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
        uc.mem_write(0x3000, struct.pack("<I", APP_BASE))  # SoftDevice info: size field (the app tests >= 0x15)
        uc.mem_write(0x3014, struct.pack("<I", 5_000_000))  # SD_VERSION: stub says "SD 50"
        uc.mem_map(RAM, RAM_SIZE)
        for base, size in ((0x40000000, 0x100000), (0x50000000, 0x1000), (0xE0000000, 0x100000),
                           (0x10000000, 0x2000), (0xF0000000, 0x1000),):
            uc.mem_map(base, size)
        uc.mem_write(RET, b"\xfe\xe7")
        sp, rst = struct.unpack_from("<II", app, 0)
        self.reg_set("sp", sp)
        self.pc = rst | 1
        self.t_us = 0.0  # virtual time at the start of the running slice
        self.slice_i0 = 0
        self.instr = 0
        self.ev = []  # heap (t, seq, fn)
        self.seq = 0
        self.regs = collections.defaultdict(int)  # UARTE registers
        self.nvic = collections.defaultdict(int)
        self.in_irq = False
        self.started = False  # set when the UART interrupt has been enabled once
        self.stop_at = 1 << 62  # instruction count at which the next scheduled event is due
        self.returned = False  # the code hook fires again until emu_stop lands
        self.sleep = False
        self.hold = False  # host holds RTS: the chip must not transmit
        self.in_q = collections.deque()  # bytes from the host, delivered as the chip accepts them
        self.fifo = collections.deque()
        self.rx_on = self.tx_on = self.rx_armed = False
        self.out = bytearray()  # raw bytes from the chip
        self.fr_hdr, self.fr_left, self.marks = b"", None, []
        self.tx_log = []  # (kind, t_us, instr): STARTTX / ENDTX
        self.ble_evts = collections.deque()
        self.ant_evts = collections.deque()
        self.svc_log = []
        self.last_error_handler = None
        try:
            self.err_pc = json.loads(Path(PRIVATE).read_text()).get("error_handler")
        except (OSError, ValueError):
            self.err_pc = None
        # `B .` handlers in the vector table (Cortex-M4 exception entries 2..15): reaching one is a hang, by address
        self.dead = set()
        for i in range(2, 16):
            a = struct.unpack_from("<I", app, 4 * i)[0] & ~1
            if a >= APP_BASE and struct.unpack_from("<H", app, a - APP_BASE)[0] == 0xE7FE:
                self.dead.add(a)
        self.masked = 0
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

    # ---- hooks
    def _unmapped(self, uc, acc, addr, size, val, ud):
        raise Stop(f"fault: unmapped access {addr:#x} at pc {self.reg('pc'):#x}")

    def _code(self, uc, addr, size, ud):
        self.instr += 1
        if self.instr >= self.stop_at:
            self.stop_at = 1 << 62
            self.instr -= 1  # this instruction runs after the restart, from its own address
            self.pc = addr | 1
            self.returned = True  # pc is already set: do not read it back
            self.uc.emu_stop()
            return
        if addr == RET:
            if not self.returned:
                self.returned = True
                self._exc_return()
        elif addr == self.err_pc:
            self.last_error_handler = addr
        if self.started and not (self.nvic["iser"] >> UARTE_IRQ) & 1:
            # the error handler enters the SoftDevice critical region (app interrupts masked) and spins; a healthy app
            # masks the UART only for a few hundred instructions. Counted in instructions, so the slice size is no input.
            self.masked += 1
            if self.masked > MASKED_LIMIT and self.hung is None:
                self.hung = (addr, f"UART interrupt masked for {MASKED_LIMIT} instructions (error handler)")
                self.uc.emu_stop()
        else:
            self.masked = 0
        if addr in self.dead and self.hung is None:
            self.hung = (addr, "reached a `B .` exception handler")
            self.uc.emu_stop()

    def _intr(self, uc, no, ud):
        pc = self.reg("pc")
        imm = self.uc.mem_read(pc - 2, 2)[0]
        self.svc(imm)

    def _mread(self, uc, acc, addr, size, val, ud):
        if addr in NVIC:
            self.wr32(addr, self.nvic["iser"] if NVIC[addr] in ("iser", "icer") else self.nvic["ispr"])
        elif UARTE0 <= addr < UARTE0 + 0x1000:
            self.wr32(addr, self.regs[addr - UARTE0])

    def _mwrite(self, uc, acc, addr, size, val, ud):
        if addr in NVIC:
            k = NVIC[addr]
            n = self.nvic
            if k == "iser":
                n["iser"] |= val
                self.started = self.started or bool((val >> UARTE_IRQ) & 1)
            elif k == "icer": n["iser"] &= ~val
            elif k == "ispr": n["ispr"] |= val
            else: n["ispr"] &= ~val
        elif UARTE0 <= addr < UARTE0 + 0x1000:
            self.uarte_write(addr - UARTE0, val)

    # ---- UARTE0 (nRF52832 PS chapter UARTE; EasyDMA, shorts ENDRX_STARTRX bit 5 / ENDRX_STOPRX bit 6)
    def uarte_write(self, off, v):
        r = self.regs
        if off == U_STARTRX and v:
            self.rx_on = True
            r[U_RXAMT] = 0
            self.arm_rx()
        elif off == U_STOPRX and v:
            if self.rx_on:
                self.rx_on = False
                self.event(EV_ENDRX)
            self.event(EV_RXTO)
        elif off == U_STARTTX and v:
            if not self.tx_on:
                self.tx_on = True
                r[U_TXAMT] = 0
                self.tx_log.append(("start", self.now(), self.instr))
                if self.fr_left is None and not self.fr_hdr:
                    self.marks.append(("fcall", self.instr))  # the app starts the next frame's header
                self.at(BYTE_US, self.tx_pump)
        elif off == U_STOPTX and v:
            self.tx_on = False
        elif off == U_INTENSET:
            r[off] |= v
            r[0x300] = r[off]
        elif off == U_INTENCLR:
            r[0x304] &= ~v
        elif 0x100 <= off < 0x160:
            r[off] = v  # clearing an event
        else:
            r[off] = v

    def event(self, off):
        self.regs[off] = 1

    def uart_irq(self):
        i = self.regs[U_INTENSET]
        return any(self.regs[o] and (i >> b) & 1 for o, b in EV_BIT.items())

    def rx_ready(self):
        """Chip's RTS: True when it accepts a byte (a receive armed, or room in the 4-byte FIFO)."""
        return self.rx_on or len(self.fifo) < 4

    def arm_rx(self):
        if not self.rx_armed:  # one pump at a time: parallel ones would deliver bytes faster than the wire
            self.rx_armed = True
            self.at(0, self.rx_pump)

    def rx_pump(self):
        self.rx_armed = False
        r = self.regs
        if self.rx_on and self.fifo is not None:
            while self.fifo and self.rx_on:
                self.rx_byte(self.fifo.popleft())
        if self.in_q and self.rx_ready():
            b = self.in_q.popleft()
            (self.rx_byte if self.rx_on else self.fifo.append)(b)
        if self.in_q:
            self.rx_armed = True
            self.at(BYTE_US, self.rx_pump)
        _ = r

    def rx_byte(self, b):
        r = self.regs
        self.uc.mem_write(r[U_RXPTR] + r[U_RXAMT], bytes([b]))
        r[U_RXAMT] += 1
        self.event(EV_RXDRDY)
        if r[U_RXAMT] >= r[U_RXMAX]:
            self.event(EV_ENDRX)
            self.rx_on = False
            if r[U_SHORTS] & 0x20:
                self.uarte_write(U_STARTRX, 1)
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
        r = self.regs
        if not self.tx_on:
            return
        if self.hold:  # CTS high: wait, no time-out (recheck when the host releases)
            self.at(BYTE_US, self.tx_pump)
            return
        b = self.uc.mem_read(r[U_TXPTR] + r[U_TXAMT], 1)
        self.out += b
        self._mark(b)
        r[U_TXAMT] += 1
        self.event(EV_TXDRDY)
        if r[U_TXAMT] >= r[U_TXMAX]:
            self.tx_on = False
            self.event(EV_ENDTX)
            self.tx_log.append(("end", self.now(), self.instr))
        else:
            self.at(BYTE_US, self.tx_pump)

    # ---- SoftDevice stubs
    def svc(self, n):
        a = [self.reg(x) for x in ("r0", "r1", "r2", "r3")]
        self.svc_log.append(n)
        ret = 0
        if n == SVC_SD_ENABLE:
            pass  # a[2] is the ANT licence key: ignored on purpose
        elif n == SVC_APP_EVT_WAIT:
            self.sleep = True
            self.pc = self.reg("pc") | 1  # the slice ends here, whatever its size: results must not depend on it
            self.returned = True
            self.uc.emu_stop()
        elif n in (SVC_BLE_ENABLE, SVC_BLE_CFG_SET):
            pass
        elif n == SVC_BLE_VERSION_GET:  # ble_version_t {u8 version, u16 company, u16 subversion}
            self.uc.mem_write(a[0], struct.pack("<BxHH", 9, 0x59, 0x1234))
        elif n == SVC_BLE_EVT_GET:
            if self.ble_evts:
                e = self.ble_evts.popleft()
                self.uc.mem_write(a[0], e)
                self.uc.mem_write(a[1], struct.pack("<H", len(e)))
            else:
                ret = NOT_FOUND
        elif n == SVC_EVT_GET:  # sd_evt_get(&id): no SoC events
            ret = NOT_FOUND
        elif n == SVC_ANT_EVENT_GET:
            if self.ant_evts:
                ch, ev, buf = self.ant_evts.popleft()
                self.uc.mem_write(a[0], bytes([ch]))
                self.uc.mem_write(a[1], bytes([ev]))
                self.uc.mem_write(a[2], buf)
            else:
                ret = NOT_FOUND
        elif 0xC0 <= n <= 0xFF:  # sd_ant_* commands (ant_interface.h ANT_SVC_BASE 0xC0): accepted, outputs left zero
            pass
        else:
            raise Stop(f"unknown SVC {n:#x} r0-r3 {[hex(x) for x in a]} pc {self.reg('pc'):#x}")
        self.reg_set("r0", ret)

    # ---- exceptions (own entry and return: Unicorn's M core has no NVIC)
    def _exc_enter(self, irq):
        sp = (self.reg("sp") - 32) & ~7
        pc = self.reg("pc")
        frame = [self.reg(x) for x in ("r0", "r1", "r2", "r3", "r12", "lr")] + [pc, self.reg("xpsr") | (1 << 24)]
        self.uc.mem_write(sp, struct.pack("<8I", *frame))
        self.reg_set("sp", sp)
        self.reg_set("lr", RET | 1)
        self.pc = self.rd32(APP_BASE + 4 * (16 + irq)) | 1
        self.in_irq = True

    def _exc_return(self):
        sp = self.reg("sp")
        f = struct.unpack("<8I", self.uc.mem_read(sp, 32))
        for x, v in zip(("r0", "r1", "r2", "r3", "r12", "lr"), f):
            self.reg_set(x, v)
        self.reg_set("sp", sp + 32)
        self.reg_set("xpsr", f[7])  # the thread's condition flags: an interrupt between cmp and branch must not change them
        self.pc = f[6] | 1
        self.in_irq = False
        self.uc.emu_stop()

    def _pending_irq(self):
        n = self.nvic
        if self.uart_irq():
            n["ispr"] |= 1 << UARTE_IRQ
        elif not (n["ispr"] >> SWI2_IRQ) & 1:
            n["ispr"] &= ~(1 << UARTE_IRQ)
        m = n["ispr"] & n["iser"]
        return (m & -m).bit_length() - 1 if m else None

    # ---- run
    def run(self, us, until=None, slice_n=2000):
        """Runs `us` of virtual time (or until `until()`); returns when done. Raises Hang or Stop."""
        from unicorn import UcError
        end = self.t_us + us
        while self.t_us < end and not (until and until()):
            self._run_events()
            if not self.in_irq and not self.reg("primask") & 1:  # cpsid i holds interrupts (the app's critical regions)
                irq = self._pending_irq()
                if irq is not None:
                    if irq != UARTE_IRQ:
                        self.nvic["ispr"] &= ~(1 << irq)
                    self.sleep = False
                    self._exc_enter(irq)
            if self.hung:
                raise Hang(f"{self.hung[1]} at {self.hung[0]:#x}")
            if self.sleep and self.started and not (self.nvic["iser"] >> UARTE_IRQ) & 1:
                # the error handler masks the app interrupts and idles (S332 critical region): a healthy app sleeps with the UART on
                raise Hang(f"asleep at {self.pc:#x} with the UART interrupt masked")
            if self.sleep:  # sd_app_evt_wait: jump to the next thing that can wake the chip
                nxt = self.ev[0][0] if self.ev else end
                self.t_us = min(max(nxt, self.t_us), end)
                if not self.ev and not self._pending_irq():
                    return
                if not self.ev or self.ev[0][0] > self.t_us:
                    continue
                continue
            # stop at the next scheduled event so an interrupt is taken on time (the gap measure needs it)
            n_max = slice_n
            self.stop_at = (self.instr + int((self.ev[0][0] - self.t_us) * CPU_MHZ) + 1) if self.ev else 1 << 62
            self.stop_at = min(self.stop_at, self.instr + int((end - self.t_us) * CPU_MHZ) + 1)  # never past `end`
            self.returned = False
            self.sleep = False  # only an sd_app_evt_wait inside this slice puts the core to sleep
            n0 = self.slice_i0 = self.instr
            try:
                self.uc.emu_start(self.pc, 0xFFFFFFF0, count=n_max)
            except UcError as e:
                raise Stop(f"fault: {e} at pc {self.reg('pc'):#x}")
            if not self.returned:
                self.pc = self.reg("pc") | 1
            did = self.instr - n0
            self.t_us += did / CPU_MHZ
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


def selftest():
    why = have()
    if why:
        print(f"skip: nrf_emu selftest ({why})")
        return 0
    e = Emu()
    e.run(50_000, until=lambda: bool(e.svc_log) and e.sleep)
    assert e.sleep, f"the app did not reach its idle wait; svcs {[hex(x) for x in e.svc_log]}"
    e.send(bytes([0, 0x65, 1]))
    e.run(5_000)
    f = e.frames()
    assert f and f[0][:2] == bytes([1, 0x65]) and len(f[0]) == 2 + 4 + 5, f"version answer {f}"
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
    print(f"ok: 8 queued events; app work between frames {g[0]} instr = {g[0] / CPU_MHZ:.0f} us at {CPU_MHZ} MHz "
          f"(CPI 1), {w[0]} instr to the first byte on the wire")
    # results do not depend on the slice size
    def script(n):
        x = Emu()
        x.run(50_000, until=lambda: bool(x.svc_log) and x.sleep, slice_n=n)
        x.send(bytes([0, 0x65, 1]))
        for i in range(10):
            x.inject_ant(1, EVENT_RX, bytes([i]), queue_only=i % 2 == 0)
            x.run(1_500, slice_n=n)
        x.run(20_000, slice_n=n)
        return bytes(x.out), x.tx_log
    assert script(2000) == script(97) == script(5000), "frames or timing depend on the slice size"
    print("ok: same frames and TX times at slice sizes 97, 2000, 5000")
    # RTS held for 200 ms while 30 events arrive: no hang, every frame in order when released
    h = Emu()
    h.run(50_000, until=lambda: bool(h.svc_log) and h.sleep)
    h.hold = True
    for i in range(30):
        h.inject_ant(1, EVENT_RX, bytes([i]))
        h.run(6_600)
    h.run(2_000)
    assert not h.frames() or len(h.out) == 0
    h.hold = False
    h.run(20_000)
    assert [p[4] for p in h.frames()] == list(range(30))
    print("ok: RTS held 200 ms, 30 events: no hang, 30 frames in order after the release")
    # the scheduler queue holds 96 events (nordic-app.md section 2): 96 in one pass are fine, 97 hang the app
    for n, hangs in ((96, False), (97, True)):
        q = Emu()
        q.run(50_000, until=lambda: bool(q.svc_log) and q.sleep)
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
        e2 = Emu()
        e2.run(50_000, until=lambda: bool(e2.svc_log) and e2.sleep, slice_n=n)
        e2.send(bytes([0, 0x01]))
        try:
            e2.run(100_000, slice_n=n)
        except Hang as h:
            seen.add((str(h), e2.instr))
        else:
            raise AssertionError("an unknown opcode did not hang the app")
        assert not e2.frames()
    assert len(seen) == 1, seen
    print(f"ok: unknown opcode hangs ({seen.pop()[0]}), identical at two slice sizes")
    return 0


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
      'X'                     restart the app (the chip's reset), run to its idle wait
      'Q'                     quit"""
    inp, out = inp or sys.stdin.buffer, out or sys.stdout.buffer
    e = [None]

    def boot():
        e[0] = Emu()
        e[0].run(50_000, until=lambda: bool(e[0].svc_log) and e[0].sleep)

    def reply(err=None, payload=b""):
        x = e[0]
        fl = (not x.rx_ready()) | x.hold << 1 | bool(x.hung) << 2
        if err:
            out.write(bytes([1, fl]) + struct.pack("<H", len(err)) + err.encode())
        else:
            out.write(bytes([0, fl]) + payload)
        out.flush()

    boot()
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
            n = struct.unpack("<H", inp.read(2))[0]
            # S132/S332 v5 ble_evt_t: {u16 evt_id 0x39 BLE_GATTC_EVT_HVX, u16 evt_len}, gattc {conn_handle, gatt_status,
            # error_handle}, hvx {handle, type, len, data} (inferred from the v5 headers, as nordic-app.md section 2)
            body = struct.pack("<HHHHBxH", 0, 0, 0, 0x0F, 1, n) + bytes(i & 255 for i in range(n))
            x.inject_ble(struct.pack("<HH", 0x39, len(body)) + body)
        elif c == b"X":
            boot()
        else:
            err = f"unknown request {c!r}"
        reply(err, payload)


if __name__ == "__main__":
    if sys.argv[1:] == ["selftest"]:
        sys.exit(selftest())
    if sys.argv[1:] == ["--serve"]:
        sys.exit(serve())
    print(__doc__)
    sys.exit(2)
