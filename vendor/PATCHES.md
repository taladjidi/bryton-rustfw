# The patched USB crates

Why: a panic marks the fault and resets the unit (AGENTS.md, "Panics"), and the USB crates held 47 panic sites in the
release image (`tools/panics.py`, 2026-10-31: 42 in usb-device, 4 in synopsys-usb-otg, 1 in usbd-serial), reached
whenever a cable is in (a desk session, a charge on a ride). Cargo.toml's `[patch.crates-io]` builds these copies
instead of the registry's; after the patch `tools/panics.py` finds no site in them.

Each folder is the crate as published, copied from `~/.cargo/registry/src/*/<crate>-<version>/` at the version
Cargo.lock pinned (LICENSE, Cargo.toml and everything else kept but `.cargo-ok`, cargo's extraction marker), with only the sites below rewritten. Every rewritten
place carries a `patched (vendor/PATCHES.md)` comment. The behaviour on well-formed traffic is unchanged: the same
descriptor bytes, the same endpoint numbers and FIFO sizes, the same control pipe states; only a case that panicked
now returns an error, or a clamped value where the function returns none. Files keep their line endings (usb-device
and usbd-serial are CRLF).

`diff -ru ~/.cargo/registry/src/*/usb-device-0.3.2 vendor/usb-device` (and the same for the other two) shows the
whole patch. Lines below are the original file's.

## Refresh (a new version of a crate)

1. Bump the version in Cargo.toml, `cargo update -p <crate>` without the patch line (comment it out), so the registry
   copy lands in `~/.cargo/registry/src/`.
2. Copy that folder over `vendor/<crate>/` whole, then apply each site below again, or drop it where upstream fixed it
   (search the new source for `[`, `unwrap`, `expect`, `panic!`, `assert!`, `unreachable!`, `unimplemented!`,
   `borrow_mut`, `borrow_ref`).
3. Restore the patch line, `cargo build --release`, `python3 tools/panics.py`: no line may name `vendor/` or a
   function of the three crates. Update this file, then `./check.sh`; a unit check (dev.py run, info, shot) before a
   ride build.

At each toolchain or dependency bump, and at a refresh: `cargo deny` matches advisories by registry source, so with
the patch the three crates (path crates) are no longer checked. Run `cargo deny check advisories` with the three
patch lines commented out, or look the three names up on RustSec (https://rustsec.org/packages/) by hand.

## usb-device 0.3.2

| Site (original) | What it did | What it does now |
| --- | --- | --- |
| macros.rs:15 `usb_log!` without log / defmt | `let _ = $arg;` ran every trace argument, and several slice buffers (`&self.buf[..count]`, control_pipe.rs:73, 151, 228) | the same lets inside `if false`: type-checked, never run |
| endpoint.rs:138-145 `Endpoint::bus` | `panic!` if the bus pointer is still null (before `freeze`) | returns `Option<&B>` |
| endpoint.rs:169, 174 `stall` / `unstall` | through `bus()` (the panic above) | nothing before `freeze` |
| endpoint.rs:195, 215 `write` / `read` | through `bus()` | `Err(InvalidState)` before `freeze` |
| control_pipe.rs:62 `data` | `&self.buf[0..self.len]` | `get`, an empty slice if `len` were past the buffer |
| control_pipe.rs:82 `handle_setup` | `&self.buf[0..count]` | `get`; a count past the buffer is ignored as a bad SETUP is |
| control_pipe.rs:136 `handle_out` | `&mut self.buf[i..]` | `get_mut`; past the buffer: `set_error` (stall), as a failed read |
| control_pipe.rs:227 `write_in_chunk` | `&buffer[self.i..(self.i + count)]` | `get`; past the buffer: `set_error`, `Err(InvalidState)` |
| descriptor.rs:66 `write` | `buf[..descriptor.len()]` (after its length check) | `get_mut`, `Err(BufferOverflow)` |
| descriptor.rs:88 `write_with` | `&mut self.buf[self.position + 2..data_end]` | `get_mut`, `Err(BufferOverflow)` |
| descriptor.rs:96-97 `write_with` | `self.buf[position]`, `[position + 1]` | one `get_mut` of the two bytes, `Err(BufferOverflow)` |
| descriptor.rs:176 `end_configuration` | `self.buf[2..4]` | `get_mut`, nothing written if the buffer were shorter |
| descriptor.rs:277 `interface_alt` | `self.buf[mark] += 1` | `get_mut`, `Err(BufferOverflow)` |
| descriptor.rs:332 `endpoint_ex` | `self.buf[mark] += 1` | `get_mut`, `Err(BufferOverflow)` |
| descriptor.rs:343-349 `endpoint_ex` | `buf[0]`..`buf[4]`, `&mut buf[5..]` (after `len < 5`) | `split_first_chunk_mut::<5>`, the same five bytes |
| descriptor.rs:361-375 `string` | `self.buf[pos]`, `[pos + 1]`, `[pos..pos + 2]`, `[self.position]` | `get_mut` each, `Err(BufferOverflow)` (also when one byte is left, where it panicked) |
| descriptor.rs:420 `BosWriter::capability` | `self.writer.buf[mark] += 1` | `get_mut`, `Err(BufferOverflow)` |
| descriptor.rs:431-437 `capability` | three bytes and `[start..start + blen]` (after its length check) | one `get_mut` split into the 3-byte head and the data |
| descriptor.rs:445 `end_bos` | `self.writer.buf[2..4]` | `get_mut`, as `end_configuration` |
| device.rs:85-115 `UsbDevice::build` | `.expect("failed to alloc control endpoint")` twice, `freeze` | returns `Result`, `?` |
| device.rs:188 `poll` | `.expect("Unknown state before suspend")` | `unwrap_or(UsbDeviceState::Default)`, the state after a reset |
| device.rs:620 `get_descriptor` | `&lang_id_bytes[..len * 2]` | `get`, `Err(BufferOverflow)` |
| device.rs:645 `get_descriptor` | `_ => unreachable!()` after 1, 2, 3 | `_ => lang.serial` (the arm only matches 1..=3) |
| device_builder.rs:107 | `build` | unchanged (now `try_build().expect`), plus `try_build -> Result<UsbDevice>`, which usb.rs calls |
| bus.rs:172-193 `freeze` | `borrow_mut` three times | `try_borrow_mut`, returns `Result<&B>` |
| bus.rs:197, 206 `interface` / `string` | `borrow_mut` (a panic after `freeze`) | `try_borrow_mut`; after `freeze` 0xff, a number no descriptor uses |
| bus.rs:225 `alloc` | `borrow_mut` (a panic after `freeze`) | `try_borrow_mut`, `Err(InvalidState)` |

Kept: `UsbBusAllocator::control` / `bulk` / `interrupt` / `isochronous` still `expect` (upstream API, documented
"Panics", and test_class.rs uses them); nothing in this firmware calls them (usb.rs and usbd-serial call `alloc`), so
they are not in the image. `panics.py` shows them again if a caller appears.

## synopsys-usb-otg 0.5.0

| Site (original) | What it did | What it does now |
| --- | --- | --- |
| lib.rs:3 | its 64 warnings printed on every build (a path dependency's lints are not capped as a registry one's) | `#![allow(warnings)]`: upstream's warnings, not ours |
| bus.rs:86 `configure_all` | `assert!(fifo_top <= FIFO_DEPTH_WORDS)` | returns before configuring the endpoints; unreachable after `alloc_ep`'s check (`allocate_tx_buffer` keeps the same total within the depth); if reached, the device does not enumerate and the link stays Some (no reset) |
| bus.rs:340, 346 `EndpointAllocator::alloc_ep` | `self.endpoints_out[i] = ..`, `endpoints_in[i]` | `get_mut`, `Err(InvalidEndpoint)` |
| bus.rs:542, 554 `write` / `read` | `endpoints_in[i]`, `endpoints_out[i]` (after the range check) | `get`, `Err(InvalidEndpoint)` |
| bus.rs:630 `poll`, high speed | `panic!("AHB frequency is too low")` below 30 MHz | TRDT 0x9 (this board: 100 MHz, BOARD.md 1) |
| bus.rs:642 `poll`, full speed | `panic!` below 14.2 MHz | TRDT 0xF, the slowest |
| bus.rs:656 `poll` | `_ => unimplemented!()` for ENUMSPD 0b10 (reserved) | taken as full speed |
| bus.rs:730-731 `poll` | `endpoints_out[epnum]` (EPNUM is 4 bits, the table 9) and `borrow_ref_mut` | `get` and `try_borrow_mut`: the packet stays in the FIFO, as for an endpoint not allocated |
| endpoint.rs:81, 173 `configure` | `panic!("Unsupported EP0 size")` | 64 bytes (`max_packet_size_0` only lets 8, 16, 32, 64 through) |
| endpoint.rs:211 `EndpointOut::read` | `borrow_ref_mut` | `try_borrow_mut`, `Err(WouldBlock)` |
| endpoint.rs:215 `buffer_state` | `borrow_ref` | `try_borrow`, `Empty` |
| endpoint_memory.rs:46-60 `read_packet` | `self.buffer[index]`, `buf[..4]`, `&mut buf[4..]`, `[..current_size]` | `get_mut(..data_size)` then by 4-byte chunks zipped with the words: the same volatile reads, the same bytes |
| endpoint_memory.rs:82 `fill_from_fifo` | `&self.buffer[..words]` (after the capacity check) | `get`, `Err(BufferOverflow)` |
| endpoint_memory.rs:153-168 `allocate_tx_buffer` | `assert!(ep_number < 9)`, `[ep_number]` twice | `get` / `get_mut`, `Err(InvalidEndpoint)` |
| endpoint_memory.rs:179 `tx_fifo_size_words` | `[ep_number]` | `get`, 0 |
| target.rs:64 `UsbRegisters::fifo` | `assert!(channel <= 15)` | `channel & 0xf`: every caller passes an allocated endpoint's number (below 9) |

## usbd-serial 0.2.2

| Site (original) | What it did | What it does now |
| --- | --- | --- |
| cdc_acm.rs:68-93 `new_with_interface_names` | `alloc.interrupt`, `alloc.bulk` twice (each an `expect`) | `try_new_with_interface_names` and `try_new` return `Result`, with `alloc(None, ..)?` in the same order (the same endpoint numbers); `new` and `new_with_interface_names` keep their API as `try_..().expect` (SerialPort uses them; this firmware does not) |
| cdc_acm.rs:248-251 `control_in`, GET_LINE_CODING | `data[0..4]`, `data[4]`..`data[6]` | `first_chunk_mut::<7>`, one array, `Err(BufferOverflow)` |
| cdc_acm.rs:281-284 `control_out`, SET_LINE_CODING | `xfer.data()[0..4].try_into().unwrap()`, `[4]`..`[6]` | a slice pattern of the first 7 bytes |
| (through usb-device) `control_out` | `xfer.data()`, control_pipe.rs:62 | fixed there |
