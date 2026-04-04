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
        }
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
                self.io[(addr & 0x3FF) as usize]
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
                self.io[(addr & 0x3FF) as usize] = value;
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
    fn rom_read() {
        let mut bus = Bus::new();
        bus.load_rom(vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(bus.read_byte(0x0800_0000), 0x01);
        assert_eq!(bus.read_word(0x0800_0000), 0x04030201); // Little-endian
    }
}
