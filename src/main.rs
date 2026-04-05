mod bus;
mod cpu;

use bus::Bus;
use cpu::{Cpu, R_PC, R_SP, R_LR};
use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut cpu = Cpu::new();
    let mut bus = Bus::new();

    if args.len() > 1 {
        // Load ROM from file
        let rom_path = &args[1];
        let rom_data = fs::read(rom_path).unwrap_or_else(|e| {
            eprintln!("Failed to load ROM '{}': {}", rom_path, e);
            std::process::exit(1);
        });
        println!("Turtle GBA Emulator");
        println!("ROM loaded: {} ({} bytes)", rom_path, rom_data.len());
        bus.load_rom(rom_data);
    } else {
        println!("Turtle GBA Emulator");
        println!("No ROM specified. Usage: turtle-gba <rom_file>");
        println!("Running built-in test program...\n");
        bus.load_rom(make_test_rom());
    }

    // Run with tracing — show each instruction as it executes
    println!("\n--- Execution Trace ---");
    println!("{:<10} {:<12} {:<30} Registers", "PC", "Hex", "Instruction");
    println!("{}", "-".repeat(80));

    let max_steps = 500_000; // Safety limit
    let verbose = args.iter().any(|a| a == "-v");
    let mut last_pc = 0u32;
    let mut stuck_count = 0u32;

    for step in 0..max_steps {
        let pc = cpu.registers[R_PC];
        let in_thumb = cpu.in_thumb_mode();

        // Detect infinite loops (same PC twice in a row = stuck)
        if pc == last_pc {
            stuck_count += 1;
            if stuck_count > 2 {
                println!("[STUCK] Infinite loop at PC=0x{:08X} after {} steps", pc, step);
                break;
            }
        } else {
            stuck_count = 0;
        }
        last_pc = pc;

        let is_halt = if in_thumb {
            bus.read_halfword(pc) == 0
        } else {
            bus.read_word(pc) == 0
        };

        if is_halt {
            println!("[HALT] instruction=0 at PC=0x{:08X} after {} steps", pc, step);
            break;
        }

        // Print first 20, mode switches, and periodic updates
        let should_print = verbose || step < 20 || step % 50000 == 0;
        if should_print {
            let desc = if in_thumb {
                format!("T:{:04X}", bus.read_halfword(pc))
            } else {
                let inst = bus.read_word(pc);
                disassemble_arm(inst)
            };
            print!("{:>6} 0x{:08X}  {:<28}", step, pc, desc);
        }

        let should_continue = cpu.step(&mut bus);

        if should_print {
            let mode_switch = in_thumb != cpu.in_thumb_mode();
            println!("R0=0x{:08X} R1=0x{:08X}{}",
                cpu.registers[0], cpu.registers[1],
                if mode_switch { "  [MODE SWITCH]" } else { "" });
        } else if in_thumb != cpu.in_thumb_mode() {
            println!("{:>6} 0x{:08X}  [MODE SWITCH → {}]",
                step, pc, if cpu.in_thumb_mode() { "THUMB" } else { "ARM" });
        }

        if !should_continue {
            println!("[HALT] CPU halted after {} steps", step + 1);
            break;
        }

        if step == max_steps - 1 {
            println!("\n[LIMIT] Stopped after {} steps (safety limit)", max_steps);
        }
    }

    // Final state
    println!("\n--- Final CPU State ---");
    for i in 0..16 {
        let name = match i {
            13 => "SP",
            14 => "LR",
            15 => "PC",
            _ => "",
        };
        if !name.is_empty() {
            println!("  R{:<2} ({:>2}): 0x{:08X}  ({})", i, name, cpu.registers[i], cpu.registers[i]);
        } else {
            println!("  R{:<2}     : 0x{:08X}  ({})", i, cpu.registers[i], cpu.registers[i]);
        }
    }
    println!("  CPSR    : 0x{:08X}  [N={} Z={} C={} V={}]",
        cpu.cpsr,
        cpu.flag_n() as u8, cpu.flag_z() as u8,
        cpu.flag_c() as u8, cpu.flag_v() as u8);
}

/// Build a minimal test ROM by hand.
/// This teaches how a GBA ROM is structured at the byte level.
fn make_test_rom() -> Vec<u8> {
    // Our test program (ARM assembly → machine code):
    //
    //   ; --- Simple counting program ---
    //   ; Counts from 1 to 10, storing sum in R2
    //
    //   entry:
    //     B start          ; Branch past the header (offset = 0x0C0 - 8) / 4
    //     ; ... 192 bytes of header (we fill with zeros, no BIOS check) ...
    //
    //   start:             ; at ROM offset 0x0C0
    //     MOV R0, #0       ; R0 = counter (starts at 0)
    //     MOV R1, #10      ; R1 = limit
    //     MOV R2, #0       ; R2 = sum
    //
    //   loop:
    //     ADD R0, R0, #1   ; counter++
    //     ADD R2, R2, R0   ; sum += counter
    //     CMP R0, R1       ; compare counter to limit
    //     BNE loop         ; if not equal, loop back
    //
    //     ; At this point: R0=10, R2=55 (1+2+3+...+10)
    //
    //     ; Store the result to IWRAM so we can verify memory works
    //     MOV R3, #0x03    ;
    //     MOV R3, R3, LSL #24 ; R3 = 0x03000000 (IWRAM base)
    //     STR R2, [R3]     ; Store sum at IWRAM[0]
    //
    //     ; Halt (instruction = 0)

    let mut rom = vec![0u8; 0x100]; // Start with 256 bytes of zeros (header space)

    // --- Offset 0x000: Entry branch ---
    // B start: branch to offset 0x0C0
    // The branch offset is: (target - instruction_addr - 8) / 4
    //   instruction_addr = 0x000
    //   target = 0x0C0
    //   offset = (0x0C0 - 0x000 - 8) / 4 = 0xB8 / 4 = 0x2E
    // Encoding: 0xEA00002E
    write_word(&mut rom, 0x000, 0xEA00_002E); // B to offset 0x0C0

    // --- Offset 0x0A0: Game title ---
    let title = b"TURTLE TEST\0";
    rom[0x0A0..0x0A0 + title.len()].copy_from_slice(title);

    // --- Offset 0x0C0: Program start ---
    let code_start = 0x0C0;
    let instructions: Vec<u32> = vec![
        0xE3A0_0000, // MOV R0, #0       ; counter = 0
        0xE3A0_100A, // MOV R1, #10      ; limit = 10
        0xE3A0_2000, // MOV R2, #0       ; sum = 0
        // loop:
        0xE280_0001, // ADD R0, R0, #1   ; counter++
        0xE082_2000, // ADD R2, R2, R0   ; sum += counter
        0xE150_0001, // CMP R0, R1       ; counter == limit?
        0x1AFF_FFFB, // BNE loop         ; offset=-5: (0x0CC - 0x0D8 - 8) / 4
        // After loop: R0=10, R2=55
        0xE3A0_3003, // MOV R3, #3
        0xE1A0_3C03, // MOV R3, R3, LSL #24  ; R3 = 0x03000000
        0xE583_2000, // STR R2, [R3]     ; IWRAM[0] = 55
        0x0000_0000, // HALT
    ];

    // Make sure ROM is big enough
    let needed = code_start + instructions.len() * 4 + 16;
    if rom.len() < needed {
        rom.resize(needed, 0);
    }

    for (i, &inst) in instructions.iter().enumerate() {
        write_word(&mut rom, code_start + i * 4, inst);
    }

    rom
}

fn write_word(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Basic ARM disassembler — just enough to read the trace.
fn disassemble_arm(instruction: u32) -> String {
    let cond = (instruction >> 28) & 0xF;
    let cond_str = match cond {
        0x0 => "EQ", 0x1 => "NE", 0x2 => "CS", 0x3 => "CC",
        0x4 => "MI", 0x5 => "PL", 0x6 => "VS", 0x7 => "VC",
        0x8 => "HI", 0x9 => "LS", 0xA => "GE", 0xB => "LT",
        0xC => "GT", 0xD => "LE", 0xE => "",   0xF => "NV",
        _ => "??",
    };

    let bits_27_26 = (instruction >> 26) & 0b11;

    match bits_27_26 {
        0b00 => {
            // Data processing
            let is_imm = (instruction >> 25) & 1 == 1;
            let opcode = (instruction >> 21) & 0xF;
            let s = if (instruction >> 20) & 1 == 1 { "S" } else { "" };
            let rn = (instruction >> 16) & 0xF;
            let rd = (instruction >> 12) & 0xF;

            let op_name = match opcode {
                0x0 => "AND", 0x1 => "EOR", 0x2 => "SUB", 0x3 => "RSB",
                0x4 => "ADD", 0x5 => "ADC", 0x6 => "SBC", 0x7 => "RSC",
                0x8 => "TST", 0x9 => "TEQ", 0xA => "CMP", 0xB => "CMN",
                0xC => "ORR", 0xD => "MOV", 0xE => "BIC", 0xF => "MVN",
                _ => "???",
            };

            let is_test = matches!(opcode, 0x8 | 0x9 | 0xA | 0xB);
            let is_mov = matches!(opcode, 0xD | 0xF);

            if is_imm {
                let imm = instruction & 0xFF;
                let rot = (instruction >> 8) & 0xF;
                let val = imm.rotate_right(rot * 2);
                if is_test {
                    format!("{}{}{} R{}, #{}", op_name, cond_str, s, rn, val)
                } else if is_mov {
                    format!("{}{}{} R{}, #{}", op_name, cond_str, s, rd, val)
                } else {
                    format!("{}{}{} R{}, R{}, #{}", op_name, cond_str, s, rd, rn, val)
                }
            } else {
                let rm = instruction & 0xF;
                let shift_type = (instruction >> 5) & 0x3;
                let shift_imm = (instruction >> 7) & 0x1F;
                let shift_name = match shift_type {
                    0 => "LSL", 1 => "LSR", 2 => "ASR", 3 => "ROR", _ => "???"
                };
                let shift_str = if shift_imm > 0 {
                    format!(", {} #{}", shift_name, shift_imm)
                } else {
                    String::new()
                };
                if is_test {
                    format!("{}{}{} R{}, R{}{}", op_name, cond_str, s, rn, rm, shift_str)
                } else if is_mov {
                    format!("{}{}{} R{}, R{}{}", op_name, cond_str, s, rd, rm, shift_str)
                } else {
                    format!("{}{}{} R{}, R{}, R{}{}", op_name, cond_str, s, rd, rn, rm, shift_str)
                }
            }
        }
        0b01 => {
            // LDR/STR
            let is_load = (instruction >> 20) & 1 == 1;
            let rn = (instruction >> 16) & 0xF;
            let rd = (instruction >> 12) & 0xF;
            let offset = instruction & 0xFFF;
            let op = if is_load { "LDR" } else { "STR" };
            if offset > 0 {
                format!("{}{} R{}, [R{}, #{}]", op, cond_str, rd, rn, offset)
            } else {
                format!("{}{} R{}, [R{}]", op, cond_str, rd, rn)
            }
        }
        0b10 => {
            if (instruction >> 25) & 1 == 1 {
                let link = if (instruction >> 24) & 1 == 1 { "BL" } else { "B" };
                let mut offset = instruction & 0x00FF_FFFF;
                if offset & 0x0080_0000 != 0 {
                    offset |= 0xFF00_0000;
                }
                let offset = ((offset << 2) as i32).wrapping_add(8);
                format!("{}{} #{:+}", link, cond_str, offset)
            } else {
                format!("LDM/STM{}", cond_str)
            }
        }
        _ => format!("??? 0x{:08X}", instruction),
    }
}
