mod bus;
mod cpu;

use bus::Bus;
use cpu::{Cpu, R_PC};

fn main() {
    let cpu = Cpu::new();
    let bus = Bus::new();

    println!("Turtle GBA Emulator");
    println!("CPU PC starts at: 0x{:08X}", cpu.registers[R_PC]);
    println!("Memory regions initialized:");
    println!("  BIOS:    {} bytes", bus.bios.len());
    println!("  EWRAM:   {} bytes", bus.ewram.len());
    println!("  IWRAM:   {} bytes", bus.iwram.len());
    println!("  VRAM:    {} bytes", bus.vram.len());
    println!("  ROM:     {} bytes (no ROM loaded)", bus.rom.len());
}
