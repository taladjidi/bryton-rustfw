#![allow(dead_code)]
use crate::target::{fifo_read_into, UsbRegisters};
use crate::UsbPeripheral;
use core::marker::PhantomData;
use core::slice;
use usb_device::{Result, UsbError};
use vcell::VolatileCell;

#[derive(Eq, PartialEq)]
pub enum EndpointBufferState {
    Empty,
    DataOut,
    DataSetup,
}

pub struct EndpointBuffer {
    buffer: &'static mut [VolatileCell<u32>],
    data_size: u16,
    has_data: bool,
    is_setup: bool,
}

impl EndpointBuffer {
    pub fn new(buffer: &'static mut [u32]) -> Self {
        Self {
            buffer: unsafe { &mut *(buffer as *mut [u32] as *mut [VolatileCell<u32>]) },
            data_size: 0,
            has_data: false,
            is_setup: false,
        }
    }

    pub fn read_packet(&mut self, buf: &mut [u8]) -> Result<usize> {
        if !self.has_data {
            return Err(UsbError::WouldBlock);
        }

        let data_size = self.data_size as usize;

        // patched (vendor/PATCHES.md): the same word-by-word copy by chunks and zip, without indexing
        let Some(dst) = buf.get_mut(..data_size) else {
            // patched (vendor/PATCHES.md): the packet is dropped; kept, RXFLVL stays pending and no WFI sleeps
            self.has_data = false;
            return Err(UsbError::BufferOverflow);
        };

        for (chunk, word) in dst.chunks_mut(4).zip(self.buffer.iter()) {
            for (d, s) in chunk.iter_mut().zip(word.get().to_ne_bytes()) {
                *d = s;
            }
        }

        self.has_data = false;

        Ok(data_size)
    }

    pub fn fill_from_fifo(
        &mut self,
        usb: UsbRegisters,
        data_size: u16,
        is_setup: bool,
    ) -> Result<()> {
        if self.has_data {
            return Err(UsbError::WouldBlock);
        }

        if data_size as usize > self.capacity() {
            return Err(UsbError::BufferOverflow);
        }

        let words = (data_size as usize + 3) / 4;
        // patched (vendor/PATCHES.md): get: the capacity check above keeps it in range
        fifo_read_into(usb, self.buffer.get(..words).ok_or(UsbError::BufferOverflow)?);

        self.is_setup = is_setup;
        self.data_size = data_size;
        self.has_data = true;

        Ok(())
    }

    pub fn state(&self) -> EndpointBufferState {
        if self.has_data {
            if self.is_setup {
                EndpointBufferState::DataSetup
            } else {
                EndpointBufferState::DataOut
            }
        } else {
            EndpointBufferState::Empty
        }
    }

    pub fn capacity(&self) -> usize {
        self.buffer.len() * 4
    }
}

impl Default for EndpointBuffer {
    fn default() -> Self {
        EndpointBuffer::new(&mut [])
    }
}

pub struct EndpointMemoryAllocator<USB> {
    next_free_offset: usize,
    max_size_words: usize,
    memory: &'static mut [u32],
    tx_fifo_size_words: [u16; 9],
    _marker: PhantomData<USB>,
}

impl<USB: UsbPeripheral> EndpointMemoryAllocator<USB> {
    pub fn new(memory: &'static mut [u32]) -> Self {
        Self {
            next_free_offset: 0,
            max_size_words: 0,
            memory,
            tx_fifo_size_words: [0; 9],
            _marker: PhantomData,
        }
    }

    pub fn allocate_rx_buffer(&mut self, size: usize) -> Result<EndpointBuffer> {
        let size_words = (size + 3) / 4;

        let offset = self.next_free_offset;
        if offset + size_words > self.memory.len() {
            return Err(UsbError::EndpointMemoryOverflow);
        }

        self.next_free_offset += size_words;
        self.max_size_words = core::cmp::max(self.max_size_words, size_words);

        let buffer = unsafe {
            let ptr = self.memory.as_mut_ptr().add(offset);
            slice::from_raw_parts_mut(ptr, size_words)
        };
        Ok(EndpointBuffer::new(buffer))
    }

    pub fn allocate_tx_buffer(&mut self, ep_number: u8, size: usize) -> Result<()> {
        let ep_number = ep_number as usize;
        // patched (vendor/PATCHES.md): an Err for a number past the table instead of assert!
        if self.tx_fifo_size_words.get(ep_number) != Some(&0) {
            return Err(UsbError::InvalidEndpoint);
        }

        let used = 30
            + self.total_rx_buffer_size_words() as usize
            + self.tx_fifo_size_words.iter().sum::<u16>() as usize;

        let size_words = core::cmp::max((size + 3) / 4, 16);
        if (used + size_words) > USB::FIFO_DEPTH_WORDS {
            return Err(UsbError::EndpointMemoryOverflow);
        }

        if let Some(slot) = self.tx_fifo_size_words.get_mut(ep_number) {
            *slot = size_words as u16;
        }

        Ok(())
    }

    /// Returns the size of memory allocated for OUT endpoints in words
    pub fn total_rx_buffer_size_words(&self) -> u16 {
        self.next_free_offset as u16
    }

    pub fn tx_fifo_size_words(&self, ep_number: usize) -> u16 {
        self.tx_fifo_size_words.get(ep_number).copied().unwrap_or(0)
    }

    pub fn max_buffer_size_words(&self) -> usize {
        self.max_size_words
    }
}
