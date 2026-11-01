// USART2 of the S510 as the radio chip's wire: the STM32H7 USART registers the firmware uses (RM0399 USART), with a
// 16-byte FIFO each way, 1 Mbaud pacing in virtual time and RTS / CTS, bridged to `tools/nrf_emu.py --serve`
// (Bryton's real nRF52 app in Unicorn) over the child's stdin / stdout. Lockstep: every pump advances the chip by the
// virtual time that passed since the last one, so the two clocks agree. PA6 (GPIO 0) is the chip's reset.
// Renode's own STM32F7_USART has no FIFO timing, no CTS / RTS and no hook for this; hence this class.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Text;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.UART
{
    // Register rules: RM0399 chapter "Universal synchronous asynchronous receiver transmitter (USART)", register
    // section: CR1 (UE 0, RE 2, TE 3, RXFNEIE 5, FIFOEN 29), CR3 (RTSE 8, CTSE 9), BRR (OVER8 = 0: baud = kernel clock /
    // BRR), ISR (ORE 3, RXFNE 5, TC 6, TXFNF 7, TEACK 21, REACK 22), ICR (ORECF bit 3, offset 0x20), RQR (RXFRQ bit 3).
    // RM0399 states that without FIFO mode RXFNE / TXFNF are RXNE / TXE and the data register is one deep.
    public class NrfUsart : IDoubleWordPeripheral, IKnownSize, IGPIOReceiver, IDisposable
    {
        // injectAfterFrames: after that frame (counted from the chip's last reset) one byte too many goes to the chip, once
        public NrfUsart(IMachine machine, string python, string tool, long injectAfterFrames = 0, bool noDrain = false)
        {
            this.machine = machine;
            IRQ = new GPIO();
            this.injectAt = injectAfterFrames;
            this.noDrain = noDrain;
            var psi = new ProcessStartInfo(python) { RedirectStandardInput = true, RedirectStandardOutput = true, UseShellExecute = false };
            psi.ArgumentList.Add("-I");
            psi.ArgumentList.Add(tool);
            psi.ArgumentList.Add("--serve");
            proc = Process.Start(psi);
            to = proc.StandardInput.BaseStream;
            from = proc.StandardOutput.BaseStream;
        }

        public GPIO IRQ { get; }
        public long Size => 0x400;

        public void Reset() { }

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
            case 0x00: return cr1;
            case 0x08: return cr3;
            case 0x0c: return brr;
            case 0x1c: // ISR; RXFNE needs UE and RE, TXFNF needs UE and TE, TC is one byte time after the last byte left
                uint isr = 0;
                if(Rx()) isr |= (rx.Count > 0 ? 1u << 5 : 0) | 1u << 22;
                if(Tx()) isr |= (tx.Count < TxDepth ? 1u << 7 : 0) | 1u << 21;
                if(ore) isr |= 1u << 3;
                if(tx.Count == 0 && Now() >= tcAt) isr |= 1u << 6;
                return isr;
            case 0x24:
                if(!Rx() || rx.Count == 0) return 0;
                var v = noDrain ? rx.Peek() : rx.Dequeue(); // noDrain: a test mode, the firmware's reads empty nothing
                Irq();
                return v;
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            switch(offset)
            {
            case 0x00:
                var ueUp = (cr1 & 1) == 0 && (value & 1) != 0;
                cr1 = value;
                if(ueUp) CheckBaud();
                Irq();
                break;
            case 0x08: cr3 = value; break;
            case 0x0c: brr = value; break;
            case 0x20: if((value & 8) != 0) ore = false; break; // ICR.ORECF
            case 0x18: if((value & 8) != 0) { rx.Clear(); Irq(); } break; // RQR.RXFRQ
            case 0x28:
                if(Tx() && tx.Count < TxDepth) { tx.Enqueue((byte)value); Wake(); }
                break;
            }
        }

        // PA6: low holds the chip in reset, the rising edge boots it again
        public void OnGPIO(int number, bool value)
        {
            if(!value) { inReset = true; return; }
            if(!inReset) return;
            inReset = false;
            resets++;
            tx.Clear(); rx.Clear(); ore = false; Irq();
            txState = 0; rxState = 0; dead = false; frames = 0;
            try { Cmd(new byte[] { (byte)'X' }); Head(); }
            catch(Exception e) { Gone(e); return; }
            this.Log(LogLevel.Warning, "nrf: chip reset #{0} at {1} us", resets, Now());
            Wake();
        }

        private ulong Now() { return (ulong)machine.ElapsedVirtualTime.TimeElapsed.TotalMicroseconds; }

        private bool Rx() { return (cr1 & 1) != 0 && (cr1 & 4) != 0; }
        private bool Tx() { return (cr1 & 1) != 0 && (cr1 & 8) != 0; }
        private int TxDepth { get { return (cr1 & (1 << 29)) != 0 ? 16 : 1; } }  // FIFOEN clear: the data register alone
        private int RxThreshold { get { return (cr1 & (1 << 29)) != 0 ? 12 : 1; } } // RTS deasserts at 3/4 full (the firmware's FIFO threshold default is inferred)

        // The kernel clock the firmware selected (RCC_D2CCIP2R USART234578SEL, RM0399 RCC; 3 = hsi_ker_ck, 64 MHz
        // divided by RCC_CR.HSIDIV) against BRR: 1 Mbaud within 2 %. Other sources (the PLLs, pclk1) are not modelled
        // by Renode's clock tree, so they count as an error: src/main.rs selects HSI.
        // Renode's RCC does not keep these registers (D2CCIP2R reads 0 whatever is written, which is also the reset value, pclk1), so a 0 means unknown: BRR is checked
        // against 64 MHz, the value src/main.rs's comment (`64 MHz / 1 Mbaud`) and BOARD.md 11 give for hsi_ker_ck. A firmware that forgot to select HSI would pass here.
        private void CheckBaud()
        {
            uint sel = machine.SystemBus.ReadDoubleWord(0x58024454) & 7, cr = machine.SystemBus.ReadDoubleWord(0x58024400);
            double hz = sel == 0 ? 64e6 : sel == 3 ? 64e6 / (1 << (int)((cr >> 3) & 3)) : 0;
            var baud = brr == 0 ? 0 : hz / brr;
            if(Math.Abs(baud - 1e6) > 0.02e6)
            {
                this.Log(LogLevel.Error, "nrf baud error: kernel clock select {0} ({1} Hz), BRR {2}: {3} baud, not 1 Mbaud", sel, hz, brr, baud);
            }
        }

        private void Irq() { IRQ.Set((cr1 & 1) != 0 && (cr1 & 0x20) != 0 && rx.Count > 0); }

        // The next pump is due in 10 us (a byte) while there is traffic (300 us after the last), else every 10 ms (20 ms
        // when the app is dead).
        private void Wake()
        {
            activeUntil = Now() + 300;
            Schedule(10);
        }

        private void Schedule(ulong us)
        {
            var due = Now() + us;
            if(scheduledAt != 0 && scheduledAt <= due) return;
            scheduledAt = due;
            var gen = ++generation;
            machine.ScheduleAction(TimeInterval.FromMicroseconds(us), _ => { if(gen == generation) { scheduledAt = 0; Pump(); } });
        }

        // A harness failure (a closed pipe, no answer within 5 s of wall time) is logged once and ends the bridge.
        private void Pump()
        {
            if(gone) return;
            try { PumpOnce(); }
            catch(Exception e) { Gone(e); }
        }

        private void Gone(Exception e)
        {
            gone = true; dead = true;
            this.Log(LogLevel.Error, "nrf harness gone: {0}", e.Message);
        }

        private void PumpOnce()
        {
            var now = Now();
            var delta = now - lastUs;
            lastUs = now;
            if(!inReset && !dead)
            {
                var send = new List<byte>();
                // one byte leaves per 10 us, only while the chip does not hold CTS (CR3.CTSE)
                var room = Math.Max(1UL, delta / 10);
                while(Tx() && tx.Count > 0 && room-- > 0 && ((cr3 & (1 << 9)) == 0 || !ctsHeld))
                {
                    var b = tx.Dequeue();
                    tcAt = now + 10;
                    send.Add(b);
                    Frame(b, send);
                }
                if(send.Count > 0)
                {
                    var m = new List<byte> { (byte)'W', (byte)send.Count, (byte)(send.Count >> 8) };
                    m.AddRange(send);
                    Cmd(m.ToArray());
                    Head();
                }
                if(delta > 0 && !dead)
                {
                    Cmd(new byte[] { (byte)'T', (byte)delta, (byte)(delta >> 8), (byte)(delta >> 16), (byte)(delta >> 24) });
                    Head();
                }
                // The wire moves one byte per 10 us of virtual time since the last pump, whatever the FIFO holds; a byte
                // that finds the FIFO full is an overrun (ORE), dropped. Only RTS (below) keeps the chip from sending.
                rxCredit = Math.Min(rxCredit + delta, 640);
                var want = Rx() ? (int)Math.Min(rxCredit / 10, 64UL) : 0;
                if(want > 0 && !dead)
                {
                    Cmd(new byte[] { (byte)'R', (byte)want, 0 });
                    if(Head())
                    {
                        var n = Rd(2);
                        var data = Rd(n[0] | n[1] << 8);
                        foreach(var b in data)
                        {
                            if(rx.Count >= (noDrain ? 4 : TxDepth)) // noDrain: a 4-byte FIFO that is never emptied, so one reply overruns it
                            {
                                if(!ore) this.Log(LogLevel.Error, "nrf overrun: a byte arrived with the RX FIFO full at {0} us", now);
                                ore = true;
                                continue;
                            }
                            rx.Enqueue(b); RxFrame(b);
                        }
                        rxCredit = data.Length < want ? 0 : rxCredit - 10UL * (ulong)data.Length;
                        if(data.Length > 0) { activeUntil = now + 300; }
                    }
                    Irq();
                }
                var hold = rx.Count >= RxThreshold && !noDrain;
                if(hold != rtsHeld && (cr3 & (1 << 8)) != 0 && !dead)
                {
                    rtsHeld = hold;
                    Cmd(new byte[] { (byte)'H', (byte)(hold ? 1 : 0) });
                    Head();
                }
            }
            Schedule(tx.Count > 0 || now < activeUntil ? 10UL : dead ? 20000UL : 10000UL);
        }

        // Host to chip framing: u16 length, payload. Logs each frame's first two bytes; in frame number injectAt one
        // byte too many goes between the header and the payload (a command longer than its header says).
        private void Frame(byte b, List<byte> send)
        {
            if(txState == 0) { txLen = b; txState = 1; }
            else if(txState == 1)
            {
                txLen |= b << 8; txState = 2; txBuf.Clear();
                if(injectAt != 0 && frames + 1 == injectAt) // between the header and the payload: the app reads a type it does not know
                {
                    injectAt = 0;
                    send.Add(0xA5);
                    this.Log(LogLevel.Warning, "nrf: one byte too many sent in frame {0}", frames + 1);
                }
                if(txLen == 0) { EndFrame(send); }
            }
            else { if(txBuf.Count < 2) txBuf.Add(b); if(--txLen == 0) { EndFrame(send); } }
        }

        private void EndFrame(List<byte> send)
        {
            txState = 0;
            frames++;
            this.Log(LogLevel.Warning, "nrf tx frame {0}: {1:X2} {2:X2}", frames, txBuf.Count > 0 ? txBuf[0] : 0, txBuf.Count > 1 ? txBuf[1] : 0);
        }

        private void RxFrame(byte b)
        {
            if(rxState == 0) { rxLen = b; rxState = 1; }
            else if(rxState == 1) { rxLen |= b << 8; rxState = 2; rxBuf.Clear(); if(rxLen == 0) rxState = 0; }
            else
            {
                if(rxBuf.Count < 6) rxBuf.Add(b);
                if(--rxLen == 0)
                {
                    rxState = 0;
                    this.Log(LogLevel.Warning, "nrf rx frame: type {0:X2} op {1:X2} status {2:X2}{3:X2}{4:X2}{5:X2}", rxBuf.Count > 0 ? rxBuf[0] : 0, rxBuf.Count > 1 ? rxBuf[1] : 0,
                        rxBuf.Count > 2 ? rxBuf[2] : 0, rxBuf.Count > 3 ? rxBuf[3] : 0, rxBuf.Count > 4 ? rxBuf[4] : 0, rxBuf.Count > 5 ? rxBuf[5] : 0);
                }
            }
        }

        private void Cmd(byte[] m) { if(gone) return; to.Write(m, 0, m.Length); to.Flush(); }

        public void Dispose()
        {
            try { Cmd(new byte[] { (byte)'Q' }); } catch(Exception) { }
            try { if(!proc.HasExited) proc.Kill(); } catch(Exception) { }
        }

        private byte[] Rd(int n)
        {
            var b = new byte[n];
            var o = 0;
            while(o < n)
            {
                var t = from.ReadAsync(b, o, n - o);
                if(!t.Wait(5000)) throw new Exception("no answer within 5 s");
                var r = t.Result;
                if(r <= 0) throw new Exception("pipe closed");
                o += r;
            }
            return b;
        }

        // [status][flags], or on status 1 a u16 length and the reason
        private bool Head()
        {
            var h = Rd(2);
            ctsHeld = (h[1] & 1) != 0;
            if(h[0] == 0) return true;
            var l = Rd(2);
            this.Log(LogLevel.Warning, "nrf: chip app stopped: {0} at {1} us", Encoding.ASCII.GetString(Rd(l[0] | l[1] << 8)), Now());
            dead = true;
            return false;
        }

        private readonly IMachine machine;
        private readonly Process proc;
        private readonly Stream to, from;
        private readonly Queue<byte> tx = new Queue<byte>(), rx = new Queue<byte>();
        private readonly List<byte> txBuf = new List<byte>(), rxBuf = new List<byte>();
        private uint cr1, cr3, brr;
        private bool ctsHeld, rtsHeld, inReset, dead, gone, ore, noDrain;
        private int txState, txLen, rxState, rxLen, resets;
        private long frames, injectAt;
        private ulong lastUs, activeUntil, scheduledAt, tcAt, rxCredit;
        private long generation;
    }
}
