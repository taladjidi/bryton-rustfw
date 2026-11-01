// SPI4 of the S510 as the GNSS receiver's wire: the STM32H7 SPI registers src/main.rs `gnss_init` / `gnss_transfer` use
// (RM0399 SPI: CR1 0x00, SR 0x14, TXDR 0x20, RXDR 0x30), bridged to the Rust model of the receiver (src/sim/gnss_serve.rs,
// started by renode/gnss_run.py, the one place the receiver's rules live) over the child's stdin / stdout. Every request
// carries the virtual time in ms. GPIO 0 is chip select (PE4, by hand in the firmware), GPIO 1 the receiver's supply (PI6).
// Renode's own STM32H7_SPI has no slave hook; hence this class (as NrfUsart.cs). No SPI clock timing: a word is
// exchanged the moment TXDR is written, which Renode's SPI cannot do better either.
using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;

namespace Antmicro.Renode.Peripherals.SPI
{
    public class GnssSpi : IDoubleWordPeripheral, IKnownSize, IGPIOReceiver, IDisposable
    {
        public GnssSpi(IMachine machine, string exe, string args)
        {
            this.machine = machine;
            var psi = new ProcessStartInfo(exe, args) { RedirectStandardInput = true, RedirectStandardOutput = true, UseShellExecute = false };
            proc = Process.Start(psi);
            to = proc.StandardInput.BaseStream;
            from = proc.StandardOutput.BaseStream;
            // libtest prints its own header first; the model says GNSS-READY when it serves
            var tail = new StringBuilder();
            while(!tail.ToString().EndsWith("GNSS-READY\n"))
            {
                var c = Rd(1)[0];
                tail.Append((char)c);
                if(tail.Length > 4096) tail.Remove(0, 2048);
            }
        }

        public long Size => 0x400;

        public void Reset() { }

        // SR (RM0399): RXP bit 0, TXP bit 1, TXC bit 12. Nothing is held in a transmit FIFO, so TXP and TXC are always set.
        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
            case 0x00: return cr1;
            case 0x14: return (rxReady ? 1u : 0) | 2 | (1u << 12);
            case 0x30: rxReady = false; return rxWord; // RXDR
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            switch(offset)
            {
            case 0x00: cr1 = value; break;
            case 0x20: // TXDR: four 8-bit frames, the first in the low byte
                var m = Req((byte)'W', 4);
                for(var i = 0; i < 4; i++) m[9 + i] = (byte)(value >> (8 * i));
                rxWord = 0xffffffff;
                if(!gone && cs)
                {
                    Send(m);
                    var r = Rd(4);
                    rxWord = (uint)(r[0] | r[1] << 8 | r[2] << 16 | r[3] << 24);
                }
                rxReady = true;
                break;
            }
        }

        // PE4 (0): the receiver listens while it is low. PI6 (1): its supply, high = on.
        public void OnGPIO(int number, bool value)
        {
            if(gone) return;
            try
            {
                if(number == 1) { var m = Req((byte)'P', 1); m[9] = (byte)(value ? 1 : 0); Send(m); Rd(1); }
                else if(!value == cs) return; // no change
                else { cs = !value; Send(Req((byte)(cs ? 'C' : 'E'), 0)); Rd(1); }
            }
            catch(Exception e)
            {
                gone = true;
                this.Log(LogLevel.Error, "gnss harness gone: {0}", e.Message);
            }
        }

        // command byte, time in ms (u64), room for `extra` bytes
        private byte[] Req(byte cmd, int extra)
        {
            var m = new byte[9 + extra];
            m[0] = cmd;
            var ms = (ulong)machine.ElapsedVirtualTime.TimeElapsed.TotalMilliseconds;
            for(var i = 0; i < 8; i++) m[1 + i] = (byte)(ms >> (8 * i));
            return m;
        }

        private void Send(byte[] m) { to.Write(m, 0, m.Length); to.Flush(); }

        public void Dispose()
        {
            try { to.WriteByte((byte)'Q'); to.Flush(); proc.WaitForExit(3000); } catch(Exception) { }
            try { if(!proc.HasExited) proc.Kill(); } catch(Exception) { }
        }

        private byte[] Rd(int n)
        {
            var b = new byte[n];
            var o = 0;
            while(o < n)
            {
                var t = from.ReadAsync(b, o, n - o);
                if(!t.Wait(10000)) throw new Exception("no answer within 10 s");
                var r = t.Result;
                if(r <= 0) throw new Exception("pipe closed (the model panicked?)");
                o += r;
            }
            return b;
        }

        private readonly IMachine machine;
        private readonly Process proc;
        private readonly Stream to, from;
        private uint cr1, rxWord;
        private bool rxReady, cs, gone;
    }
}
