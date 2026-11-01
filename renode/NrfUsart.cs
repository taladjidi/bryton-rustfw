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
    public class NrfUsart : IDoubleWordPeripheral, IKnownSize, IGPIOReceiver
    {
        // injectAfterFrames: after that frame (counted from the chip's last reset) one byte too many goes to the chip, once
        public NrfUsart(IMachine machine, string python, string tool, long injectAfterFrames = 0)
        {
            this.machine = machine;
            IRQ = new GPIO();
            this.injectAt = injectAfterFrames;
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
            case 0x1c: // ISR: RXFNE 5, TC 6, TXFNF 7, TEACK 21, REACK 22
                return (uint)((rx.Count > 0 ? 1 << 5 : 0) | (tx.Count == 0 ? 1 << 6 : 0) | (tx.Count < 16 ? 1 << 7 : 0) | (1 << 21) | (1 << 22));
            case 0x24:
                var v = rx.Count > 0 ? rx.Dequeue() : (byte)0;
                Irq();
                return v;
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            switch(offset)
            {
            case 0x00: cr1 = value; Irq(); break;
            case 0x08: cr3 = value; break;
            case 0x18: if((value & 8) != 0) { rx.Clear(); Irq(); } break; // RQR.RXFRQ
            case 0x28:
                if(tx.Count < 16) { tx.Enqueue((byte)value); Wake(); }
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
            tx.Clear(); rx.Clear(); Irq();
            txState = 0; rxState = 0; dead = false; frames = 0;
            Cmd(new byte[] { (byte)'X' });
            Head();
            this.Log(LogLevel.Warning, "nrf: chip reset #{0} at {1} us", resets, Now());
            Wake();
        }

        private ulong Now() { return (ulong)machine.ElapsedVirtualTime.TimeElapsed.TotalMicroseconds; }

        private void Irq() { IRQ.Set((cr1 & 1) != 0 && (cr1 & 0x20) != 0 && rx.Count > 0); }

        // The next pump is due in 10 us (a byte) while there is traffic, else every 2 ms.
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

        private void Pump()
        {
            var now = Now();
            var delta = now - lastUs;
            lastUs = now;
            if(!inReset && !dead)
            {
                var send = new List<byte>();
                // one byte leaves per 10 us, only while the chip does not hold CTS (CR3.CTSE)
                var room = Math.Max(1UL, delta / 10);
                while(tx.Count > 0 && room-- > 0 && ((cr3 & (1 << 9)) == 0 || !ctsHeld))
                {
                    var b = tx.Dequeue();
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
                var free = 16 - rx.Count;
                if(free > 0 && !dead)
                {
                    Cmd(new byte[] { (byte)'R', (byte)free, 0 });
                    if(Head())
                    {
                        var n = Rd(2);
                        var data = Rd(n[0] | n[1] << 8);
                        foreach(var b in data) { rx.Enqueue(b); RxFrame(b); }
                        if(data.Length > 0) { activeUntil = now + 300; }
                    }
                    Irq();
                }
                var hold = rx.Count >= 12;
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

        private void Cmd(byte[] m) { to.Write(m, 0, m.Length); to.Flush(); }

        private byte[] Rd(int n)
        {
            var b = new byte[n];
            var o = 0;
            while(o < n)
            {
                var r = from.Read(b, o, n - o);
                if(r <= 0) throw new Exception("nrf harness gone");
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
        private uint cr1, cr3;
        private bool ctsHeld, rtsHeld, inReset, dead;
        private int txState, txLen, rxState, rxLen, resets;
        private long frames, injectAt;
        private ulong lastUs, activeUntil, scheduledAt;
        private long generation;
    }
}
