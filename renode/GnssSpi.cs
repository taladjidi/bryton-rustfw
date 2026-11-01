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
            var psi = new ProcessStartInfo(exe, args) { RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false };
            proc = Process.Start(psi);
            // a model panic's message goes to Renode's log (gnss.log)
            proc.ErrorDataReceived += (o, e) => { if(e.Data != null) this.Log(LogLevel.Warning, "gnss model: {0}", e.Data); };
            proc.BeginErrorReadLine();
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

        // Register model, RM0399 "SPI" (STM32H7): CR1 0x00 (SPE 0, CSTART 9, SSI 12), CR2 0x04 (TSIZE 15:0), CFG1 0x08, CFG2 0x0C
        // (SSM 26, MASTER 22), SR 0x14 (RXP 0, TXP 1, EOT 3, OVR 6, MODF 9, TXC 12, CTSIZE 31:16), IFCR 0x18 (EOTC 3, OVRC 6,
        // MODFC 9), TXDR 0x20, RXDR 0x30. CFG1, CFG2 and CR2 take writes only while SPE = 0 (RM0399, inferred for CR2).
        // The data path needs SPE, CSTART and MASTER. A mode fault (SSM with SSI = 0 and MASTER set) sets MODF and clears
        // MASTER and SPE: the SPI1 incident of 2026-10-23 (BOARD.md 6). Nothing is held in a transmit FIFO, so TXP and TXC
        // follow the data path being on. Not modelled: SPI clock timing, the FIFO threshold, CRC, the other flags.
        private bool On => (cr1 & 1) != 0 && (cr1 & CSTART) != 0 && (cfg2 & MASTER) != 0;

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
            case 0x00: return cr1;
            case 0x04: return cr2;
            case 0x08: return cfg1;
            case 0x0c: return cfg2;
            case 0x14:
                return (rxReady ? 1u : 0) | (On ? 2u | (1u << 12) : 0) | (eot ? 8u : 0) | (ovr ? 1u << 6 : 0)
                    | (modf ? 1u << 9 : 0) | ((uint)(framesLeft & 0xffff) << 16);
            case 0x30: // RXDR
                if(!rxReady)
                {
                    this.Log(LogLevel.Warning, "gnss spi: RXDR read without RXP (0xffffffff)");
                    return 0xffffffff;
                }
                rxReady = false;
                return rxWord;
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            var spe = (cr1 & 1) != 0;
            switch(offset)
            {
            case 0x00:
                if(spe && (value & 1) == 0) { rxReady = false; eot = false; value &= ~CSTART; framesLeft = 0; } // disabled
                cr1 = value & ~(modf ? 1u : 0);
                if((value & CSTART) != 0 && On) framesLeft = (int)(cr2 & 0xffff);
                Fault();
                break;
            case 0x04: if(!spe) cr2 = value; break;
            case 0x08: if(!spe) cfg1 = value; break;
            case 0x0c: if(!spe) { cfg2 = value; Fault(); } break;
            case 0x18: // IFCR
                if((value & 8) != 0) eot = false;
                if((value & (1u << 6)) != 0) ovr = false;
                if((value & (1u << 9)) != 0) modf = false;
                break;
            case 0x20: // TXDR: four 8-bit frames, the first in the low byte
                if(!On) { this.Log(LogLevel.Warning, "gnss spi: TXDR written with SPE/CSTART/MASTER not all set: dropped"); break; }
                if(rxReady) { ovr = true; this.Log(LogLevel.Warning, "gnss spi: TXDR written while RXP is set: OVR"); break; }
                if((cr2 & 0xffff) != 0 && framesLeft < 4) break; // TSIZE frames done: EOT already set
                var m = Req((byte)'W', 4);
                for(var i = 0; i < 4; i++) m[9 + i] = (byte)(value >> (8 * i));
                rxWord = 0xffffffff;
                try
                {
                    if(!gone && cs)
                    {
                        Send(m);
                        var r = Rd(4);
                        rxWord = (uint)(r[0] | r[1] << 8 | r[2] << 16 | r[3] << 24);
                    }
                }
                catch(Exception e)
                {
                    gone = true;
                    this.Log(LogLevel.Error, "gnss harness gone: {0}", e.Message);
                }
                rxReady = true;
                if((cr2 & 0xffff) != 0)
                {
                    framesLeft -= 4;
                    if(framesLeft < 4) { eot = true; cr1 &= ~CSTART; }
                }
                break;
            }
        }

        // MODF: SSM = 1, SSI = 0 and MASTER = 1 (internal NSS low while master): MASTER and SPE are cleared by hardware.
        private void Fault()
        {
            if((cfg2 & (1u << 26)) != 0 && (cr1 & (1u << 12)) == 0 && (cfg2 & MASTER) != 0)
            {
                modf = true; cfg2 &= ~MASTER; cr1 &= ~1u;
                this.Log(LogLevel.Warning, "gnss spi: mode fault (MASTER set while SSI = 0): MASTER and SPE cleared");
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
        private const uint CSTART = 1u << 9, MASTER = 1u << 22;
        private uint cr1, cr2, cfg1, cfg2, rxWord;
        private int framesLeft;
        private bool rxReady, cs, gone, modf, ovr, eot;
    }
}
