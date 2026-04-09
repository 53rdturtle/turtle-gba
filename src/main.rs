mod bus;
mod cpu;
mod ppu;

use bus::Bus;
use cpu::{Cpu, R_PC, R_SP, R_LR};
use ppu::{SCREEN_WIDTH, SCREEN_HEIGHT};
use std::env;
use std::fs;

use minifb::{Key, Window, WindowOptions};

/// Cycles per frame on the GBA: 228 scanlines × 1232 cycles each = 280,896.
const CYCLES_PER_FRAME: u32 = 280_896;

/// How many GBA frames to simulate per window frame.
/// minifb only updates key state on window.update(), so this must be 1
/// for responsive input. (Higher values cause input lag because
/// is_key_down() returns stale data between update() calls.)
const FRAMES_PER_RENDER: u32 = 1;

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut cpu = Cpu::new();
    let mut bus = Bus::new();

    println!("Turtle GBA Emulator");

    // Try to load BIOS from common locations
    let bios_paths = ["roms/gba_bios.bin", "gba_bios.bin", "bios.bin"];
    let mut bios_loaded = false;
    for path in &bios_paths {
        if let Ok(bios_data) = fs::read(path) {
            if bios_data.len() == 0x4000 {
                println!("BIOS loaded: {} ({} bytes)", path, bios_data.len());
                bus.load_bios(bios_data);
                bios_loaded = true;
                break;
            }
        }
    }
    // Also check --bios flag
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--bios" && i + 1 < args.len() {
            let bios_data = fs::read(&args[i + 1]).unwrap_or_else(|e| {
                eprintln!("Failed to load BIOS '{}': {}", args[i + 1], e);
                std::process::exit(1);
            });
            println!("BIOS loaded: {} ({} bytes)", args[i + 1], bios_data.len());
            bus.load_bios(bios_data);
            bios_loaded = true;
            i += 2;
            continue;
        }
        i += 1;
    }

    // Find ROM path (first non-flag argument)
    let rom_path = args.iter().skip(1)
        .filter(|a| *a != "--bios" && *a != "-v" && *a != "--headless" && *a != "--auto-test")
        .filter(|a| !args.iter().any(|b| b == "--bios" && args.iter().position(|x| x == b).map(|p| args.get(p + 1)) == Some(Some(a))))
        .next();

    if let Some(rom_path) = rom_path {
        let rom_data = fs::read(rom_path).unwrap_or_else(|e| {
            eprintln!("Failed to load ROM '{}': {}", rom_path, e);
            std::process::exit(1);
        });
        println!("ROM loaded: {} ({} bytes)", rom_path, rom_data.len());
        bus.load_rom(rom_data);

        if bios_loaded {
            cpu.registers[R_PC] = 0x0800_0000;
            cpu.cpsr = 0x1F;
            cpu.registers[R_SP] = 0x0300_7F00;
            cpu.registers[R_LR] = 0x0800_0000;
            cpu.set_mode_sp(0x13, 0x0300_7FE0);
            cpu.set_mode_sp(0x12, 0x0300_7FA0);
            println!("  BIOS skip: CPU initialized to post-boot state");
        }
    } else {
        println!("No ROM specified. Usage: turtle-gba <rom_file> [--bios <bios.bin>]");
        println!("Running built-in test program...\n");
        bus.load_rom(make_test_rom());
    }

    let headless = args.iter().any(|a| a == "--headless");
    let auto_test = args.iter().any(|a| a == "--auto-test");

    if auto_test {
        run_auto_test(&mut cpu, &mut bus);
    } else if headless {
        run_headless(&mut cpu, &mut bus, &args);
    } else {
        run_with_window(&mut cpu, &mut bus);
    }
}

/// Run with a graphical window — the normal emulator mode.
///
/// Each iteration: execute one frame's worth of CPU cycles, then render
/// the screen from VRAM/palette state and display it in the window.
fn run_with_window(cpu: &mut Cpu, bus: &mut Bus) {
    let mut window = Window::new(
        "Turtle GBA",
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
        WindowOptions {
            scale: minifb::Scale::X4, // 240×160 is tiny, scale up 4×
            ..WindowOptions::default()
        },
    ).expect("Failed to create window");

    // Cap at ~60 FPS (GBA runs at ~59.73 Hz)
    window.set_target_fps(60);

    let mut fps_timer = std::time::Instant::now();
    let mut fps_frame_count: u64 = 0;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        for _ in 0..FRAMES_PER_RENDER {
            update_keyinput(&window, bus);
            // Use bus.cycles as the single timing source — this stays in sync
            // even when BIOS calls (Halt, VBlankIntrWait) advance cycles internally.
            let frame_start = bus.cycles;
            while bus.cycles.wrapping_sub(frame_start) < CYCLES_PER_FRAME {
                let cycles = cpu.step(bus);
                if cycles == 0 {
                    break; // Halt
                }
                bus.tick(cycles);
            }

            // Advance timing to frame boundary
            while bus.cycles.wrapping_sub(frame_start) < CYCLES_PER_FRAME {
                bus.tick(1);
            }
        }

        // Render and display
        let framebuf = ppu::render_frame(&bus.vram, &bus.palette, &bus.oam, &bus.io);
        window.update_with_buffer(&framebuf, SCREEN_WIDTH, SCREEN_HEIGHT)
            .expect("Failed to update window");

        fps_frame_count += 1;

        // Update FPS in window title every second
        let elapsed = fps_timer.elapsed();
        if elapsed.as_secs_f64() >= 1.0 {
            let fps = fps_frame_count as f64 / elapsed.as_secs_f64();
            window.set_title(&format!("Turtle GBA — {:.0} FPS", fps));
            fps_frame_count = 0;
            fps_timer = std::time::Instant::now();
        }
    }
}

/// Map PC keyboard keys to GBA button bits and update KEYINPUT.
///
/// GBA KEYINPUT (0x04000130) is active-low: 0 = pressed, 1 = released.
/// We start with all bits set (nothing pressed) and clear bits for held keys.
///
/// Mapping:
///   Z → A        Arrow keys → D-pad
///   X → B        Enter → Start
///   A → L        Backspace → Select
///   S → R
fn update_keyinput(window: &Window, bus: &mut Bus) {
    let key_map: &[(Key, u16)] = &[
        (Key::Z,         1 << 0),  // A
        (Key::X,         1 << 1),  // B
        (Key::Backspace, 1 << 2),  // Select
        (Key::Enter,     1 << 3),  // Start
        (Key::Right,     1 << 4),  // Right
        (Key::Left,      1 << 5),  // Left
        (Key::Up,        1 << 6),  // Up
        (Key::Down,      1 << 7),  // Down
        (Key::S,         1 << 8),  // R shoulder
        (Key::A,         1 << 9),  // L shoulder
    ];

    let mut keyinput: u16 = 0x03FF; // All released
    for &(key, bit) in key_map {
        if window.is_key_down(key) {
            keyinput &= !bit; // Clear bit = pressed
        }
    }
    bus.keyinput = keyinput;
}

/// Automated test mode: navigate through all armwrestler pages,
/// saving a screenshot of each.
///
/// Armwrestler navigation: Down = move cursor, Start = select page.
/// We simulate button presses at timed intervals to cycle through pages.
fn run_auto_test(cpu: &mut Cpu, bus: &mut Bus) {
    println!("\n--- Auto-Test Mode ---");
    println!("Navigating armwrestler test pages...\n");

    let wait = 60;   // Frames to wait for rendering
    let hold = 10;   // Frames to hold a button

    // Armwrestler menu items (cursor starts on first item)
    let menu_names = [
        "01-arm-alu",
        "02-arm-ldr-str",
        "03-arm-ldm-stm",
        "04-thumb-alu",
        "05-thumb-ldr-str",
        "06-thumb-ldm-stm",
    ];

    // Let the menu render
    run_frames(cpu, bus, 0x03FF, wait * 2);

    for (i, name) in menu_names.iter().enumerate() {
        // Press Start to enter this menu item
        press_button(cpu, bus, 1 << 3, hold, wait);

        // Capture the test page
        let framebuf = ppu::render_frame(&bus.vram, &bus.palette, &bus.oam, &bus.io);
        let filename = format!("test_{}.bmp", name);
        save_bmp(&filename, &framebuf);
        println!("  Saved {} (menu item {})", filename, i + 1);

        // Press Select to go back to menu
        press_button(cpu, bus, 1 << 2, hold, wait);

        // Move cursor down to next item (unless last)
        if i + 1 < menu_names.len() {
            press_button(cpu, bus, 1 << 7, hold, wait); // Down
        }
    }

    println!("\nDone! {} pages captured.", menu_names.len());
}

/// Press a GBA button (given as a bitmask), hold for `hold` frames, release and wait `wait` frames.
fn press_button(cpu: &mut Cpu, bus: &mut Bus, button_bit: u16, hold: u32, wait: u32) {
    run_frames(cpu, bus, 0x03FF & !button_bit, hold);
    run_frames(cpu, bus, 0x03FF, wait);
}

/// Run N GBA frames with a given KEYINPUT state.
fn run_frames(cpu: &mut Cpu, bus: &mut Bus, keyinput: u16, num_frames: u32) {
    for _ in 0..num_frames {
        bus.keyinput = keyinput;
        let frame_start = bus.cycles;
        while bus.cycles.wrapping_sub(frame_start) < CYCLES_PER_FRAME {
            let cycles = cpu.step(bus);
            if cycles == 0 { break; }
            bus.tick(cycles);
        }
        // Advance timing to frame boundary
        while bus.cycles.wrapping_sub(frame_start) < CYCLES_PER_FRAME {
            bus.tick(1);
        }
    }
}

/// Save a 240×160 framebuffer as a BMP file (for debugging).
fn save_bmp(path: &str, framebuf: &[u32]) {
    use std::io::Write;
    let w = SCREEN_WIDTH as u32;
    let h = SCREEN_HEIGHT as u32;
    let row_size = w * 3;
    let padding = (4 - (row_size % 4)) % 4;
    let pixel_data_size = (row_size + padding) * h;
    let file_size = 54 + pixel_data_size;

    let mut f = match fs::File::create(path) {
        Ok(f) => f,
        Err(e) => { eprintln!("Failed to save BMP: {}", e); return; }
    };

    // BMP header (14 bytes)
    f.write_all(b"BM").ok();
    f.write_all(&file_size.to_le_bytes()).ok();
    f.write_all(&[0u8; 4]).ok(); // reserved
    f.write_all(&54u32.to_le_bytes()).ok(); // pixel data offset

    // DIB header (40 bytes)
    f.write_all(&40u32.to_le_bytes()).ok();
    f.write_all(&w.to_le_bytes()).ok();
    f.write_all(&h.to_le_bytes()).ok();
    f.write_all(&1u16.to_le_bytes()).ok(); // planes
    f.write_all(&24u16.to_le_bytes()).ok(); // bits per pixel
    f.write_all(&[0u8; 24]).ok(); // compression, size, resolution, colors

    // Pixel data (BMP stores bottom-to-top)
    for y in (0..h as usize).rev() {
        for x in 0..w as usize {
            let rgb = framebuf[y * w as usize + x];
            let r = ((rgb >> 16) & 0xFF) as u8;
            let g = ((rgb >> 8) & 0xFF) as u8;
            let b = (rgb & 0xFF) as u8;
            f.write_all(&[b, g, r]).ok(); // BMP order: BGR
        }
        for _ in 0..padding {
            f.write_all(&[0]).ok();
        }
    }
}

/// Headless mode — run without a window (for testing/CI).
fn run_headless(cpu: &mut Cpu, bus: &mut Bus, args: &[String]) {
    println!("\n--- Execution Trace (headless) ---");

    let max_steps = 50_000_000;
    let verbose = args.iter().any(|a| a == "-v");
    let mut last_pc = 0u32;
    let mut stuck_count = 0u32;

    for step in 0..max_steps {
        let pc = cpu.registers[R_PC];
        let in_thumb = cpu.in_thumb_mode();

        if step % 10_000_000 == 0 && step > 0 {
            println!("[{}M] PC=0x{:08X} {}",
                step / 1_000_000, pc, if in_thumb { "THUMB" } else { "ARM" });
        }

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

        let should_print = verbose || step < 20;
        if should_print {
            let desc = if in_thumb {
                format!("T:{:04X}", bus.read_halfword(pc))
            } else {
                let inst = bus.read_word(pc);
                disassemble_arm(inst)
            };
            println!("{:>6} 0x{:08X}  {:<28} R0=0x{:08X} R1=0x{:08X} CPSR=0x{:08X}",
                step, pc, desc, cpu.registers[0], cpu.registers[1], cpu.cpsr);
        }

        let cycles = cpu.step(bus);
        if cycles == 0 {
            println!("[HALT] instruction=0 at PC=0x{:08X} after {} steps", pc, step);
            break;
        }
        bus.tick(cycles);

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

    // Dump both VRAM frame buffers as images
    let dispcnt = (bus.io[0] as u16) | ((bus.io[1] as u16) << 8);
    println!("\n--- Display State ---");
    println!("  DISPCNT: 0x{:04X} (mode={})", dispcnt, dispcnt & 7);

    // Save a snapshot of the display state
    let dispcnt = (bus.io[0] as u16) | ((bus.io[1] as u16) << 8);
    println!("\n--- Display State ---");
    println!("  DISPCNT: 0x{:04X} (mode={})", dispcnt, dispcnt & 7);
    let framebuf = ppu::render_frame(&bus.vram, &bus.palette, &bus.oam, &bus.io);
    save_bmp("headless.bmp", &framebuf);
    println!("  Saved headless.bmp");
}

/// Build a minimal test ROM by hand.
fn make_test_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x100];

    write_word(&mut rom, 0x000, 0xEA00_002E); // B to offset 0x0C0

    let title = b"TURTLE TEST\0";
    rom[0x0A0..0x0A0 + title.len()].copy_from_slice(title);

    let code_start = 0x0C0;
    let instructions: Vec<u32> = vec![
        0xE3A0_0000, // MOV R0, #0
        0xE3A0_100A, // MOV R1, #10
        0xE3A0_2000, // MOV R2, #0
        0xE280_0001, // ADD R0, R0, #1
        0xE082_2000, // ADD R2, R2, R0
        0xE150_0001, // CMP R0, R1
        0x1AFF_FFFB, // BNE loop
        0xE3A0_3003, // MOV R3, #3
        0xE1A0_3C03, // MOV R3, R3, LSL #24
        0xE583_2000, // STR R2, [R3]
        0x0000_0000, // HALT
    ];

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
