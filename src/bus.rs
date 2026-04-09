/// The memory bus — the highway connecting the CPU to everything else.
///
/// On the real GBA, when the CPU accesses an address, the bus routes it
/// to the right place based on the address range:
///
///   0x00000000 - 0x00003FFF  BIOS (16 KB) — startup code, built into the GBA
///   0x02000000 - 0x0203FFFF  EWRAM (256 KB) — external work RAM (slow)
///   0x03000000 - 0x03007FFF  IWRAM (32 KB) — internal work RAM (fast)
///   0x04000000 - 0x040003FE  I/O Registers — control hardware
///   0x05000000 - 0x050003FF  Palette RAM (1 KB) — colors
///   0x06000000 - 0x06017FFF  VRAM (96 KB) — graphics data
///   0x07000000 - 0x070003FF  OAM (1 KB) — sprite attributes
///   0x08000000 - 0x09FFFFFF  ROM (up to 32 MB) — the game cartridge
///
/// For now, we use simple arrays. Later we'll add proper I/O register handling.

/// DMA channel state. The GBA has 4 DMA channels that can copy memory
/// independently of the CPU. Each channel has source/destination addresses,
/// a word count, and a control register that configures the transfer.
#[derive(Clone)]
pub struct DmaChannel {
    /// Source address (written by CPU)
    pub sad: u32,
    /// Destination address (written by CPU)
    pub dad: u32,
    /// Word count (written by CPU)
    pub count: u16,
    /// Control register (CNT_H)
    pub control: u16,
    /// Internal source address (used during transfer, may auto-increment)
    pub internal_sad: u32,
    /// Internal destination address
    pub internal_dad: u32,
}

impl DmaChannel {
    fn new() -> Self {
        DmaChannel {
            sad: 0, dad: 0, count: 0, control: 0,
            internal_sad: 0, internal_dad: 0,
        }
    }

    fn enabled(&self) -> bool { self.control & (1 << 15) != 0 }
    fn transfer_32bit(&self) -> bool { self.control & (1 << 10) != 0 }
    fn start_timing(&self) -> u16 { (self.control >> 12) & 3 }
    fn dest_control(&self) -> u16 { (self.control >> 5) & 3 }
    fn src_control(&self) -> u16 { (self.control >> 7) & 3 }
    fn repeat(&self) -> bool { self.control & (1 << 9) != 0 }
}

/// Timer state. The GBA has 4 hardware timers that count up and
/// generate interrupts on overflow. They can be clocked by the
/// system clock (with prescaler) or cascaded from the previous timer.
#[derive(Clone)]
pub struct Timer {
    /// Current counter value (16-bit, counts up toward 0xFFFF)
    pub counter: u16,
    /// Reload value — loaded into counter on overflow or enable
    pub reload: u16,
    /// Prescaler accumulator — tracks sub-tick fractional cycles
    pub prescaler_counter: u32,
}

impl Timer {
    fn new() -> Self {
        Timer { counter: 0, reload: 0, prescaler_counter: 0 }
    }
}

pub struct Bus {
    /// BIOS memory — 16 KB
    pub bios: Vec<u8>,

    /// External Work RAM — 256 KB, general purpose but slower
    pub ewram: Vec<u8>,

    /// Internal Work RAM — 32 KB, fast (on-chip)
    pub iwram: Vec<u8>,

    /// I/O registers — 1 KB (we'll flesh this out later)
    pub io: Vec<u8>,

    /// Palette RAM — 1 KB (stores color palettes for graphics)
    pub palette: Vec<u8>,

    /// Video RAM — 96 KB (stores tiles, maps, bitmaps)
    pub vram: Vec<u8>,

    /// Object Attribute Memory — 1 KB (sprite positions, sizes, etc.)
    pub oam: Vec<u8>,

    /// Game ROM — loaded from file, up to 32 MB
    pub rom: Vec<u8>,

    /// CPU cycle counter — used for PPU timing
    pub cycles: u32,

    /// KEYINPUT register (0x04000130) — active-low button state.
    pub keyinput: u16,

    /// 4 DMA channels (0-3), priority: 0 > 1 > 2 > 3
    pub dma: [DmaChannel; 4],

    /// 4 hardware timers (TM0-TM3)
    pub timers: [Timer; 4],
}

impl Bus {
    pub fn new() -> Self {
        Bus {
            bios: vec![0; 0x4000],       // 16 KB
            ewram: vec![0; 0x40000],     // 256 KB
            iwram: vec![0; 0x8000],      // 32 KB
            io: vec![0; 0x400],          // 1 KB
            palette: vec![0; 0x400],     // 1 KB
            vram: vec![0; 0x18000],      // 96 KB
            oam: vec![0; 0x400],         // 1 KB
            rom: Vec::new(),             // Loaded later
            cycles: 0,
            keyinput: 0x03FF,            // All buttons released
            dma: [DmaChannel::new(), DmaChannel::new(), DmaChannel::new(), DmaChannel::new()],
            timers: [Timer::new(), Timer::new(), Timer::new(), Timer::new()],
        }
    }

    /// Load BIOS data into the BIOS region.
    pub fn load_bios(&mut self, data: Vec<u8>) {
        let len = data.len().min(self.bios.len());
        self.bios[..len].copy_from_slice(&data[..len]);
    }

    /// Read a single byte from the given address.
    /// The address determines which memory region we access.
    /// We mask the address to stay within each region's bounds.
    pub fn read_byte(&self, addr: u32) -> u8 {
        match addr {
            // BIOS
            0x0000_0000..=0x0000_3FFF => {
                self.bios[(addr & 0x3FFF) as usize]
            }

            // EWRAM — mirrored every 256 KB
            0x0200_0000..=0x02FF_FFFF => {
                self.ewram[(addr & 0x3FFFF) as usize]
            }

            // IWRAM — mirrored every 32 KB
            0x0300_0000..=0x03FF_FFFF => {
                self.iwram[(addr & 0x7FFF) as usize]
            }

            // I/O Registers
            0x0400_0000..=0x0400_03FE => {
                let offset = (addr & 0x3FF) as usize;
                match offset {
                    // KEYINPUT — return live button state, not the io array
                    0x130 => self.keyinput as u8,
                    0x131 => (self.keyinput >> 8) as u8,
                    // Timer counter reads — return the live counter, not IO array
                    0x100 => self.timers[0].counter as u8,
                    0x101 => (self.timers[0].counter >> 8) as u8,
                    0x104 => self.timers[1].counter as u8,
                    0x105 => (self.timers[1].counter >> 8) as u8,
                    0x108 => self.timers[2].counter as u8,
                    0x109 => (self.timers[2].counter >> 8) as u8,
                    0x10C => self.timers[3].counter as u8,
                    0x10D => (self.timers[3].counter >> 8) as u8,
                    _ => self.io[offset],
                }
            }

            // Palette RAM
            0x0500_0000..=0x05FF_FFFF => {
                self.palette[(addr & 0x3FF) as usize]
            }

            // VRAM
            0x0600_0000..=0x06FF_FFFF => {
                // VRAM is 96 KB but mirrored; addresses above 0x17FFF wrap
                let offset = (addr & 0x1FFFF) as usize;
                let offset = if offset >= 0x18000 { offset - 0x8000 } else { offset };
                self.vram[offset]
            }

            // OAM
            0x0700_0000..=0x07FF_FFFF => {
                self.oam[(addr & 0x3FF) as usize]
            }

            // ROM — up to 32 MB across three mirrors
            0x0800_0000..=0x0DFF_FFFF => {
                let offset = (addr & 0x01FF_FFFF) as usize;
                if offset < self.rom.len() {
                    self.rom[offset]
                } else {
                    0 // Reading past ROM returns 0 (simplified)
                }
            }

            // Unmapped — the real GBA returns open bus values, we return 0 for now
            _ => 0,
        }
    }

    /// Write a single byte to the given address.
    pub fn write_byte(&mut self, addr: u32, value: u8) {
        match addr {
            // BIOS is read-only — writes are ignored
            0x0000_0000..=0x0000_3FFF => {}

            // EWRAM
            0x0200_0000..=0x02FF_FFFF => {
                self.ewram[(addr & 0x3FFFF) as usize] = value;
            }

            // IWRAM
            0x0300_0000..=0x03FF_FFFF => {
                self.iwram[(addr & 0x7FFF) as usize] = value;
            }

            // I/O Registers
            0x0400_0000..=0x0400_03FE => {
                let offset = (addr & 0x3FF) as usize;

                // IF register (0x04000202-203): write-1-to-clear (acknowledge interrupts)
                if offset == 0x202 || offset == 0x203 {
                    self.io[offset] &= !value;
                    return;
                }

                // Timer reload registers — writes set reload, not the live counter
                match offset {
                    0x100 => { self.timers[0].reload = (self.timers[0].reload & 0xFF00) | value as u16; return; }
                    0x101 => { self.timers[0].reload = (self.timers[0].reload & 0x00FF) | ((value as u16) << 8); return; }
                    0x104 => { self.timers[1].reload = (self.timers[1].reload & 0xFF00) | value as u16; return; }
                    0x105 => { self.timers[1].reload = (self.timers[1].reload & 0x00FF) | ((value as u16) << 8); return; }
                    0x108 => { self.timers[2].reload = (self.timers[2].reload & 0xFF00) | value as u16; return; }
                    0x109 => { self.timers[2].reload = (self.timers[2].reload & 0x00FF) | ((value as u16) << 8); return; }
                    0x10C => { self.timers[3].reload = (self.timers[3].reload & 0xFF00) | value as u16; return; }
                    0x10D => { self.timers[3].reload = (self.timers[3].reload & 0x00FF) | ((value as u16) << 8); return; }
                    _ => {}
                }

                // Timer control register writes — detect enable edge (0→1)
                match offset {
                    0x102 | 0x106 | 0x10A | 0x10E => {
                        let timer_idx = (offset - 0x102) / 4;
                        let old_enable = self.io[offset] & 0x80 != 0;
                        let new_enable = value & 0x80 != 0;
                        self.io[offset] = value;
                        // On 0→1 enable transition: reload counter and reset prescaler
                        if !old_enable && new_enable {
                            self.timers[timer_idx].counter = self.timers[timer_idx].reload;
                            self.timers[timer_idx].prescaler_counter = 0;
                        }
                        return;
                    }
                    _ => {}
                }

                self.io[offset] = value;

                // Check for DMA control register writes.
                match offset {
                    0x0BB => self.on_dma_control_write(0),
                    0x0C7 => self.on_dma_control_write(1),
                    0x0D3 => self.on_dma_control_write(2),
                    0x0DF => self.on_dma_control_write(3),
                    _ => {}
                }
            }

            // Palette RAM
            0x0500_0000..=0x05FF_FFFF => {
                self.palette[(addr & 0x3FF) as usize] = value;
            }

            // VRAM
            0x0600_0000..=0x06FF_FFFF => {
                let offset = (addr & 0x1FFFF) as usize;
                let offset = if offset >= 0x18000 { offset - 0x8000 } else { offset };
                self.vram[offset] = value;
            }

            // OAM
            0x0700_0000..=0x07FF_FFFF => {
                self.oam[(addr & 0x3FF) as usize] = value;
            }

            // ROM is read-only — writes ignored
            0x0800_0000..=0x0DFF_FFFF => {}

            _ => {} // Unmapped, ignore
        }
    }

    /// Read a 32-bit word (4 bytes). ARM instructions are 32 bits wide.
    /// The GBA is little-endian: the lowest address holds the least significant byte.
    pub fn read_word(&self, addr: u32) -> u32 {
        let b0 = self.read_byte(addr) as u32;
        let b1 = self.read_byte(addr + 1) as u32;
        let b2 = self.read_byte(addr + 2) as u32;
        let b3 = self.read_byte(addr + 3) as u32;
        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }

    /// Write a 32-bit word (4 bytes), little-endian.
    pub fn write_word(&mut self, addr: u32, value: u32) {
        self.write_byte(addr, value as u8);
        self.write_byte(addr + 1, (value >> 8) as u8);
        self.write_byte(addr + 2, (value >> 16) as u8);
        self.write_byte(addr + 3, (value >> 24) as u8);
    }

    /// Read a 16-bit halfword (2 bytes). THUMB instructions are 16 bits wide.
    pub fn read_halfword(&self, addr: u32) -> u16 {
        let b0 = self.read_byte(addr) as u16;
        let b1 = self.read_byte(addr + 1) as u16;
        b0 | (b1 << 8)
    }

    /// Write a 16-bit halfword (2 bytes), little-endian.
    pub fn write_halfword(&mut self, addr: u32, value: u16) {
        self.write_byte(addr, value as u8);
        self.write_byte(addr + 1, (value >> 8) as u8);
    }

    /// Load a ROM file into memory.
    pub fn load_rom(&mut self, data: Vec<u8>) {
        self.rom = data;
    }

    /// Called when the high byte of a DMA channel's CNT_H register is written.
    /// Reads the full DMA configuration from the I/O array, latches addresses,
    /// and runs the transfer immediately if start timing = 0.
    fn on_dma_control_write(&mut self, ch: usize) {
        let base = 0x0B0 + ch * 12; // I/O offset for this channel

        // Read SAD, DAD, CNT_L, CNT_H from the I/O array
        let sad = self.io_read_u32(base);
        let dad = self.io_read_u32(base + 4);
        let count = self.io_read_u16(base + 8);
        let control = self.io_read_u16(base + 10);

        let was_enabled = self.dma[ch].enabled();
        self.dma[ch].control = control;

        // On rising edge of enable bit: latch addresses and count
        if !was_enabled && self.dma[ch].enabled() {
            // Mask source/dest addresses to valid ranges
            let sad_mask: u32 = if ch == 0 { 0x07FF_FFFF } else { 0x0FFF_FFFF };
            let dad_mask: u32 = if ch == 3 { 0x0FFF_FFFF } else { 0x07FF_FFFF };
            self.dma[ch].sad = sad & sad_mask;
            self.dma[ch].dad = dad & dad_mask;
            self.dma[ch].count = count;
            self.dma[ch].internal_sad = self.dma[ch].sad;
            self.dma[ch].internal_dad = self.dma[ch].dad;

            // Start timing 0 = immediate transfer
            if self.dma[ch].start_timing() == 0 {
                self.run_dma(ch);
            }
        }
    }

    /// Execute a DMA transfer for the given channel.
    fn run_dma(&mut self, ch: usize) {
        let transfer_32 = self.dma[ch].transfer_32bit();
        let src_ctrl = self.dma[ch].src_control();
        let dst_ctrl = self.dma[ch].dest_control();
        let mut src = self.dma[ch].internal_sad;
        let mut dst = self.dma[ch].internal_dad;

        // Word count: 0 means max (0x4000 for DMA0-2, 0x10000 for DMA3)
        let count = if self.dma[ch].count == 0 {
            if ch == 3 { 0x10000u32 } else { 0x4000u32 }
        } else {
            self.dma[ch].count as u32
        };

        let step: u32 = if transfer_32 { 4 } else { 2 };

        for _ in 0..count {
            if transfer_32 {
                let val = self.read_word(src);
                self.write_word(dst, val);
            } else {
                let val = self.read_halfword(src);
                self.write_halfword(dst, val);
            }

            // Advance source address
            match src_ctrl {
                0 => src = src.wrapping_add(step),  // Increment
                1 => src = src.wrapping_sub(step),  // Decrement
                2 => {}                              // Fixed
                _ => {}                              // Prohibited (3)
            }

            // Advance destination address
            match dst_ctrl {
                0 | 3 => dst = dst.wrapping_add(step), // Increment (3 = inc/reload)
                1 => dst = dst.wrapping_sub(step),      // Decrement
                2 => {}                                  // Fixed
                _ => {}
            }
        }

        // Save internal addresses for repeat DMA
        self.dma[ch].internal_sad = src;
        self.dma[ch].internal_dad = if dst_ctrl == 3 {
            self.dma[ch].dad // Reload destination for mode 3
        } else {
            dst
        };

        // If not repeat, disable the channel
        if !self.dma[ch].repeat() || self.dma[ch].start_timing() == 0 {
            self.dma[ch].control &= !(1 << 15); // Clear enable bit
            // Write back to I/O so CPU reads reflect disabled state
            let ctrl_offset = 0x0B0 + ch * 12 + 10;
            self.io[ctrl_offset] = self.dma[ch].control as u8;
            self.io[ctrl_offset + 1] = (self.dma[ch].control >> 8) as u8;
        }
    }

    /// Helper: read a 32-bit value from the I/O register array
    fn io_read_u32(&self, offset: usize) -> u32 {
        self.io[offset] as u32
            | (self.io[offset + 1] as u32) << 8
            | (self.io[offset + 2] as u32) << 16
            | (self.io[offset + 3] as u32) << 24
    }

    /// Helper: read a 16-bit value from the I/O register array
    fn io_read_u16(&self, offset: usize) -> u16 {
        self.io[offset] as u16 | (self.io[offset + 1] as u16) << 8
    }

    /// Check and run VBlank-triggered DMA (called when entering VBlank)
    pub fn check_vblank_dma(&mut self) {
        for ch in 0..4 {
            if self.dma[ch].enabled() && self.dma[ch].start_timing() == 1 {
                self.run_dma(ch);
            }
        }
    }

    /// Check and run HBlank-triggered DMA (called when entering HBlank)
    pub fn check_hblank_dma(&mut self) {
        for ch in 0..4 {
            if self.dma[ch].enabled() && self.dma[ch].start_timing() == 2 {
                self.run_dma(ch);
            }
        }
    }

    /// Advance the PPU timing by one CPU instruction worth of cycles.
    ///
    /// The GBA PPU draws one scanline every 1232 cycles (960 visible + 272 HBlank).
    /// After 160 visible scanlines, it enters VBlank (scanlines 160-227).
    /// Total frame = 228 scanlines * 1232 cycles = 280,896 cycles.
    ///
    /// For now, we update two I/O registers:
    ///   0x04000004 (DISPSTAT) — bit 0 = in VBlank, bit 1 = in HBlank
    ///   0x04000006 (VCOUNT)   — current scanline (0-227)
    pub fn tick(&mut self, cycles: u32) {
        let old_dispstat = self.io[0x004];
        self.cycles = self.cycles.wrapping_add(cycles);

        let scanline_cycles = self.cycles % 280896;
        let current_line = (scanline_cycles / 1232) as u8;
        let line_cycle = scanline_cycles % 1232;

        // Update VCOUNT (scanline counter at IO offset 0x006)
        self.io[0x006] = current_line;
        self.io[0x007] = 0;

        // Update DISPSTAT (display status at IO offset 0x004)
        let in_vblank = current_line >= 160;
        let in_hblank = line_cycle >= 960;
        let mut dispstat = self.io[0x004] & 0xF8; // Preserve upper bits (interrupt enables)
        if in_vblank { dispstat |= 0x01; }
        if in_hblank { dispstat |= 0x02; }
        // Bit 2: VCount match (compare current line with target in bits 15-8)
        let vcount_target = self.io[0x005];
        if current_line == vcount_target { dispstat |= 0x04; }
        self.io[0x004] = dispstat;

        // Detect rising edges for VBlank/HBlank
        let was_vblank = old_dispstat & 0x01 != 0;
        let was_hblank = old_dispstat & 0x02 != 0;

        // Trigger DMA on VBlank/HBlank rising edges
        if in_vblank && !was_vblank { self.check_vblank_dma(); }
        if in_hblank && !was_hblank { self.check_hblank_dma(); }

        // Set interrupt flags (IF at 0x04000202) on rising edges
        // DISPSTAT bits 3-5 enable the corresponding interrupts
        if in_vblank && !was_vblank && (dispstat & 0x08 != 0) {
            // VBlank IRQ (IF bit 0)
            self.io[0x202] |= 0x01;
        }
        if in_hblank && !was_hblank && (dispstat & 0x10 != 0) {
            // HBlank IRQ (IF bit 1)
            self.io[0x202] |= 0x02;
        }
        let was_vcmatch = old_dispstat & 0x04 != 0;
        let is_vcmatch = dispstat & 0x04 != 0;
        if is_vcmatch && !was_vcmatch && (dispstat & 0x20 != 0) {
            // VCount match IRQ (IF bit 2)
            self.io[0x202] |= 0x04;
        }

        // --- Timers ---
        self.tick_timers(cycles);
    }

    /// Advance all enabled timers by the given number of CPU cycles.
    ///
    /// Each timer has a prescaler that divides the system clock:
    ///   0 = 1:1 (every cycle), 1 = 1:64, 2 = 1:256, 3 = 1:1024
    ///
    /// Cascade timers (bit 2 of control) don't count cycles — they only
    /// increment when the previous timer overflows.
    fn tick_timers(&mut self, cycles: u32) {
        let prescaler_divs: [u32; 4] = [1, 64, 256, 1024];

        for i in 0..4 {
            let control = self.io[0x102 + i * 4];
            let enabled = control & 0x80 != 0;
            let cascade = control & 0x04 != 0;

            if !enabled || (cascade && i > 0) {
                continue; // Not running, or cascade (handled by previous timer's overflow)
            }

            let prescaler = prescaler_divs[(control & 0x3) as usize];
            self.timers[i].prescaler_counter += cycles;

            // How many timer ticks have elapsed?
            let ticks = self.timers[i].prescaler_counter / prescaler;
            self.timers[i].prescaler_counter %= prescaler;

            if ticks > 0 {
                self.timer_add(i, ticks);
            }
        }
    }

    /// Add `ticks` to timer `idx`, handling overflow, reload, IRQ, and cascade.
    fn timer_add(&mut self, idx: usize, ticks: u32) {
        let old = self.timers[idx].counter as u32;
        let new_val = old + ticks;

        if new_val > 0xFFFF {
            // Overflow — how many times?
            let remaining = new_val - 0x10000;
            let reload = self.timers[idx].reload as u32;
            // For very large tick counts, there could be multiple overflows
            let range = 0x10000 - reload as u32;
            let overflows = if range > 0 { 1 + remaining / range } else { 1 };

            // Reload the counter (accounting for leftover ticks past the last overflow)
            self.timers[idx].counter = if range > 0 {
                (reload + remaining % range) as u16
            } else {
                reload as u16
            };

            // Fire IRQ if enabled (control bit 6)
            let control = self.io[0x102 + idx * 4];
            if control & 0x40 != 0 {
                // Timer IRQ: IF bits 3-6 for timers 0-3
                self.io[0x202] |= 1 << (3 + idx);
            }

            // Cascade: if the next timer exists and is in cascade mode, feed it
            if idx < 3 {
                let next_control = self.io[0x102 + (idx + 1) * 4];
                let next_enabled = next_control & 0x80 != 0;
                let next_cascade = next_control & 0x04 != 0;
                if next_enabled && next_cascade {
                    self.timer_add(idx + 1, overflows);
                }
            }
        } else {
            self.timers[idx].counter = new_val as u16;
        }
    }

    /// Check if any interrupt is pending and should be delivered.
    /// Returns true if IME is set, CPSR I-bit is clear, and (IE & IF) != 0.
    pub fn irq_pending(&self) -> bool {
        let ime = self.io[0x208] & 1; // Interrupt Master Enable
        if ime == 0 { return false; }
        let ie = (self.io[0x200] as u16) | ((self.io[0x201] as u16) << 8);
        let if_ = (self.io[0x202] as u16) | ((self.io[0x203] as u16) << 8);
        (ie & if_) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_write_iwram() {
        let mut bus = Bus::new();
        bus.write_byte(0x0300_0000, 0x42);
        assert_eq!(bus.read_byte(0x0300_0000), 0x42);
    }

    #[test]
    fn iwram_is_mirrored() {
        // IWRAM repeats every 32 KB, so these should be the same location
        let mut bus = Bus::new();
        bus.write_byte(0x0300_0000, 0xAB);
        assert_eq!(bus.read_byte(0x0300_8000), 0xAB); // Mirror!
    }

    #[test]
    fn read_write_word_little_endian() {
        let mut bus = Bus::new();
        // Write 0xDEADBEEF to EWRAM
        bus.write_word(0x0200_0000, 0xDEAD_BEEF);

        // Little-endian: lowest byte first
        assert_eq!(bus.read_byte(0x0200_0000), 0xEF); // Least significant
        assert_eq!(bus.read_byte(0x0200_0001), 0xBE);
        assert_eq!(bus.read_byte(0x0200_0002), 0xAD);
        assert_eq!(bus.read_byte(0x0200_0003), 0xDE); // Most significant

        // Reading the word back should give us the original
        assert_eq!(bus.read_word(0x0200_0000), 0xDEAD_BEEF);
    }

    #[test]
    fn bios_is_read_only() {
        let mut bus = Bus::new();
        bus.write_byte(0x0000_0000, 0xFF);
        assert_eq!(bus.read_byte(0x0000_0000), 0); // Write was ignored
    }

    #[test]
    fn keyinput_defaults_all_released() {
        let bus = Bus::new();
        // All buttons released: KEYINPUT = 0x03FF
        assert_eq!(bus.read_byte(0x0400_0130), 0xFF);
        assert_eq!(bus.read_byte(0x0400_0131), 0x03);
        assert_eq!(bus.read_halfword(0x0400_0130), 0x03FF);
    }

    #[test]
    fn dma3_immediate_copy() {
        let mut bus = Bus::new();
        // Write source data to EWRAM at 0x02000000
        bus.write_word(0x0200_0000, 0xDEAD_BEEF);
        bus.write_word(0x0200_0004, 0xCAFE_BABE);

        // Configure DMA3 (offset 0x0D4):
        //   SAD = 0x02000000, DAD = 0x03000000, count = 2
        //   Control: 32-bit, immediate, enable
        bus.write_word(0x0400_00D4, 0x0200_0000);  // SAD
        bus.write_word(0x0400_00D8, 0x0300_0000);  // DAD
        bus.write_halfword(0x0400_00DC, 2);          // Count = 2 words
        bus.write_halfword(0x0400_00DE, 0x8400);     // Enable | 32-bit | immediate

        // Verify data was copied to IWRAM
        assert_eq!(bus.read_word(0x0300_0000), 0xDEAD_BEEF);
        assert_eq!(bus.read_word(0x0300_0004), 0xCAFE_BABE);

        // DMA should have auto-disabled (immediate, no repeat)
        assert_eq!(bus.read_halfword(0x0400_00DE) & 0x8000, 0);
    }

    #[test]
    fn dma3_halfword_fill() {
        let mut bus = Bus::new();
        // Source: a single halfword in EWRAM
        bus.write_halfword(0x0200_0000, 0x1234);

        // DMA3: 16-bit, fixed source, increment dest, count=4
        bus.write_word(0x0400_00D4, 0x0200_0000);  // SAD
        bus.write_word(0x0400_00D8, 0x0300_0000);  // DAD
        bus.write_halfword(0x0400_00DC, 4);          // 4 halfwords
        bus.write_halfword(0x0400_00DE, 0x8100);     // Enable | 16-bit | src_fixed(2<<7=0x100)

        // All 4 halfwords should be the same value (fixed source)
        for i in 0..4u32 {
            assert_eq!(bus.read_halfword(0x0300_0000 + i * 2), 0x1234,
                "halfword at offset {} wrong", i);
        }
    }

    #[test]
    fn keyinput_reflects_pressed_buttons() {
        let mut bus = Bus::new();
        // Press A (bit 0) and Start (bit 3): clear those bits
        bus.keyinput = 0x03FF & !(1 << 0) & !(1 << 3); // 0x03F6
        assert_eq!(bus.read_halfword(0x0400_0130), 0x03F6);
    }

    #[test]
    fn rom_read() {
        let mut bus = Bus::new();
        bus.load_rom(vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(bus.read_byte(0x0800_0000), 0x01);
        assert_eq!(bus.read_word(0x0800_0000), 0x04030201); // Little-endian
    }
}
