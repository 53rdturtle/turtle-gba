/// The ARM7TDMI CPU — the brain of the GBA.
///
/// The real chip runs at 16.78 MHz (about 16 million instructions per second).
/// We model its state as registers + a reference to memory.

/// The CPU has different operating modes (e.g., normal, interrupt handling).
/// Each mode has its own banked copies of some registers.
/// For now, we start with just "System" mode — the normal one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CpuMode {
    System,
    // We'll add more modes later: FIQ, IRQ, Supervisor, Abort, Undefined
}

/// The CPU state: 16 registers + status register.
pub struct Cpu {
    /// R0 through R15. Index 15 is the Program Counter (PC).
    /// Each register is 32 bits — that's what "32-bit CPU" means.
    /// u32 in Rust is an unsigned 32-bit integer: 0 to 4,294,967,295.
    pub registers: [u32; 16],

    /// Current Program Status Register.
    /// Bit 31: N (Negative)
    /// Bit 30: Z (Zero)
    /// Bit 29: C (Carry)
    /// Bit 28: V (Overflow)
    /// Bits 4-0: Mode bits (which CPU mode we're in)
    /// We store the whole thing as a u32 and use bit manipulation to read flags.
    pub cpsr: u32,

    /// Current operating mode
    pub mode: CpuMode,
}

// Named indices for clarity — so we write R_PC instead of magic number 15.
pub const R_SP: usize = 13;
pub const R_LR: usize = 14;
pub const R_PC: usize = 15;

// CPSR flag bit positions
pub const CPSR_N: u32 = 1 << 31; // Negative
pub const CPSR_Z: u32 = 1 << 30; // Zero
pub const CPSR_C: u32 = 1 << 29; // Carry
pub const CPSR_V: u32 = 1 << 28; // Overflow

impl Cpu {
    /// Create a new CPU in its startup state.
    /// On real hardware, the GBA starts executing at address 0x08000000 (the ROM),
    /// but first the BIOS runs from 0x00000000. We'll start at the ROM entry point.
    pub fn new() -> Self {
        let mut cpu = Cpu {
            registers: [0; 16],
            cpsr: 0,
            mode: CpuMode::System,
        };

        // The PC starts at the beginning of ROM
        cpu.registers[R_PC] = 0x0800_0000;

        // The stack pointer is conventionally initialized to the top of IWRAM
        cpu.registers[R_SP] = 0x0300_7F00;

        cpu
    }

    // --- Flag helpers ---
    // These read/write individual bits in the CPSR.
    // Bit manipulation is fundamental to emulation — hardware is all bits!

    /// Is the Zero flag set?
    pub fn flag_z(&self) -> bool {
        (self.cpsr & CPSR_Z) != 0
    }

    /// Is the Negative flag set?
    pub fn flag_n(&self) -> bool {
        (self.cpsr & CPSR_N) != 0
    }

    /// Is the Carry flag set?
    pub fn flag_c(&self) -> bool {
        (self.cpsr & CPSR_C) != 0
    }

    /// Is the Overflow flag set?
    pub fn flag_v(&self) -> bool {
        (self.cpsr & CPSR_V) != 0
    }

    /// Set or clear a flag. `mask` is which bit, `value` is on/off.
    pub fn set_flag(&mut self, mask: u32, value: bool) {
        if value {
            self.cpsr |= mask; // Turn the bit ON
        } else {
            self.cpsr &= !mask; // Turn the bit OFF
        }
    }

    // --- Barrel Shifter ---
    // ARM's secret weapon: the second operand can be shifted for free.
    // The shifter returns both the shifted value AND a carry-out bit
    // (which may update the C flag).

    /// Apply a shift operation to a value.
    /// Returns (shifted_value, carry_out).
    /// `shift_type`: 0=LSL, 1=LSR, 2=ASR, 3=ROR
    /// `amount`: how many bits to shift (0-31 for immediate, 0-255 for register)
    fn barrel_shift(&self, value: u32, shift_type: u32, amount: u32) -> (u32, bool) {
        let carry_in = self.flag_c(); // Current carry flag as default

        if amount == 0 {
            // Shift by 0 is a special case for each type:
            // LSL #0: no change, carry unchanged
            // LSR #0: actually means LSR #32 (value becomes 0, carry = bit 31)
            // ASR #0: actually means ASR #32 (all bits become sign bit)
            // ROR #0: actually means RRX (rotate right by 1 through carry)
            // But when encoded as immediate shift of 0, LSL #0 = identity
            return (value, carry_in);
        }

        match shift_type {
            0 => { // LSL — Logical Shift Left
                if amount >= 32 {
                    let carry = if amount == 32 { value & 1 != 0 } else { false };
                    (0, carry)
                } else {
                    let carry = (value >> (32 - amount)) & 1 != 0;
                    (value << amount, carry)
                }
            }
            1 => { // LSR — Logical Shift Right
                if amount >= 32 {
                    let carry = if amount == 32 { (value >> 31) & 1 != 0 } else { false };
                    (0, carry)
                } else {
                    let carry = (value >> (amount - 1)) & 1 != 0;
                    (value >> amount, carry)
                }
            }
            2 => { // ASR — Arithmetic Shift Right (preserves sign)
                if amount >= 32 {
                    // All bits become the sign bit
                    let sign = (value as i32) >> 31;
                    (sign as u32, (value >> 31) & 1 != 0)
                } else {
                    let carry = (value >> (amount - 1)) & 1 != 0;
                    // Cast to i32 so >> preserves the sign bit (arithmetic shift)
                    ((value as i32 >> amount) as u32, carry)
                }
            }
            3 => { // ROR — Rotate Right
                if amount == 0 {
                    // RRX: rotate right by 1 through carry
                    let carry = value & 1 != 0;
                    let result = (value >> 1) | ((carry_in as u32) << 31);
                    (result, carry)
                } else {
                    let amount = amount % 32;
                    if amount == 0 {
                        (value, (value >> 31) & 1 != 0)
                    } else {
                        let carry = (value >> (amount - 1)) & 1 != 0;
                        (value.rotate_right(amount), carry)
                    }
                }
            }
            _ => unreachable!(),
        }
    }

    /// Decode operand2 for data processing instructions.
    /// Returns (value, carry_out).
    fn decode_operand2(&self, instruction: u32, is_immediate: bool) -> (u32, bool) {
        if is_immediate {
            // Immediate: 8-bit value rotated right by (rotate * 2)
            let imm = instruction & 0xFF;
            let rotate = (instruction >> 8) & 0xF;
            if rotate == 0 {
                (imm, self.flag_c())
            } else {
                let result = imm.rotate_right(rotate * 2);
                let carry = (result >> 31) & 1 != 0;
                (result, carry)
            }
        } else {
            // Register with optional shift
            let rm = (instruction & 0xF) as usize;
            let rm_val = self.registers[rm];

            let shift_type = (instruction >> 5) & 0x3;
            let shift_by_reg = (instruction >> 4) & 1 == 1;

            let amount = if shift_by_reg {
                // Shift amount from register (Rs), using bottom byte only
                let rs = ((instruction >> 8) & 0xF) as usize;
                self.registers[rs] & 0xFF
            } else {
                // Shift amount is a 5-bit immediate
                (instruction >> 7) & 0x1F
            };

            self.barrel_shift(rm_val, shift_type, amount)
        }
    }

    // --- Condition checking ---
    // Every ARM instruction has a 4-bit condition code in bits 31-28.
    // The CPU checks flags BEFORE executing — if the condition fails, the
    // instruction is skipped (treated as a no-op). This is unique to ARM!

    /// Check if a condition code is met, based on current CPSR flags.
    /// Returns true if the instruction should execute.
    pub fn condition_met(&self, cond: u32) -> bool {
        match cond {
            0x0 => self.flag_z(),                              // EQ: equal (Z set)
            0x1 => !self.flag_z(),                             // NE: not equal (Z clear)
            0x2 => self.flag_c(),                              // CS: carry set
            0x3 => !self.flag_c(),                             // CC: carry clear
            0x4 => self.flag_n(),                              // MI: minus/negative
            0x5 => !self.flag_n(),                             // PL: plus/positive
            0x6 => self.flag_v(),                              // VS: overflow set
            0x7 => !self.flag_v(),                             // VC: overflow clear
            0x8 => self.flag_c() && !self.flag_z(),            // HI: unsigned higher
            0x9 => !self.flag_c() || self.flag_z(),            // LS: unsigned lower or same
            0xA => self.flag_n() == self.flag_v(),             // GE: signed >=
            0xB => self.flag_n() != self.flag_v(),             // LT: signed <
            0xC => !self.flag_z() && (self.flag_n() == self.flag_v()), // GT: signed >
            0xD => self.flag_z() || (self.flag_n() != self.flag_v()), // LE: signed <=
            0xE => true,                                       // AL: always (most common)
            0xF => true,                                       // Unconditional (ARMv5+, treat as always for now)
            _ => unreachable!(),
        }
    }

    // --- Fetch-Decode-Execute ---

    /// Execute one CPU step: fetch an instruction, decode it, execute it.
    /// Returns true if execution should continue, false to halt.
    pub fn step(&mut self, bus: &mut crate::bus::Bus) -> bool {
        // FETCH: read 32-bit instruction at the PC
        let pc = self.registers[R_PC];
        let instruction = bus.read_word(pc);

        // Advance PC to next instruction (4 bytes ahead for ARM mode)
        self.registers[R_PC] = pc.wrapping_add(4);

        // If instruction is 0, treat as halt (no real instruction is all zeros in practice)
        if instruction == 0 {
            return false;
        }

        // Check condition code (bits 31-28) — should we even execute this?
        let cond = (instruction >> 28) & 0xF;
        if !self.condition_met(cond) {
            return true; // Skip this instruction, but keep running
        }

        // DECODE: determine instruction type from bit patterns
        // ARM encoding uses several bits to identify the instruction category
        let bits_27_26 = (instruction >> 26) & 0b11;
        let bit_25 = (instruction >> 25) & 1;

        match bits_27_26 {
            0b00 => {
                // Data processing (ALU operations): MOV, ADD, SUB, CMP, AND, ORR, etc.
                self.execute_data_processing(instruction, bit_25);
            }
            0b01 => {
                // Single data transfer: LDR (load from memory), STR (store to memory)
                self.execute_single_transfer(instruction, bus);
            }
            0b10 => {
                if (instruction >> 25) & 1 == 1 {
                    // Branch (B) and Branch with Link (BL)
                    self.execute_branch(instruction);
                }
                // else: block data transfer (LDM/STM) — later
            }
            0b11 => {
                // Coprocessor / SWI — later
            }
            _ => unreachable!(),
        }

        true
    }

    // --- Data Processing (ALU) ---
    // These are the math/logic instructions: MOV, ADD, SUB, AND, ORR, CMP, etc.
    //
    // Encoding:
    //   bits 24-21: opcode (which operation)
    //   bit 20: S flag (should we update CPSR flags?)
    //   bits 19-16: Rn (first operand register)
    //   bits 15-12: Rd (destination register)
    //   bits 11-0: operand2 (either immediate value or shifted register)
    //   bit 25 (I): if 1, operand2 is an immediate; if 0, it's a register

    fn execute_data_processing(&mut self, instruction: u32, is_immediate: u32) {
        let opcode = (instruction >> 21) & 0xF;
        let set_flags = (instruction >> 20) & 1 == 1;
        let rn = ((instruction >> 16) & 0xF) as usize;
        let rd = ((instruction >> 12) & 0xF) as usize;

        // Get operand2 value using the barrel shifter
        let (op2, shift_carry) = self.decode_operand2(instruction, is_immediate == 1);

        let op1 = self.registers[rn];

        // Execute the operation based on opcode
        let result = match opcode {
            0x0 => op1 & op2,                         // AND
            0x1 => op1 ^ op2,                         // EOR (exclusive or)
            0x2 => op1.wrapping_sub(op2),             // SUB
            0x3 => op2.wrapping_sub(op1),             // RSB (reverse subtract)
            0x4 => op1.wrapping_add(op2),             // ADD
            0x5 => {                                   // ADC (add with carry)
                let c = if self.flag_c() { 1u32 } else { 0 };
                op1.wrapping_add(op2).wrapping_add(c)
            }
            0x6 => {                                   // SBC (subtract with carry)
                let c = if self.flag_c() { 1u32 } else { 0 };
                op1.wrapping_sub(op2).wrapping_add(c).wrapping_sub(1)
            }
            0x7 => {                                   // RSC (reverse sub with carry)
                let c = if self.flag_c() { 1u32 } else { 0 };
                op2.wrapping_sub(op1).wrapping_add(c).wrapping_sub(1)
            }
            0x8 => { /* TST */ op1 & op2 }            // Test (AND but don't store)
            0x9 => { /* TEQ */ op1 ^ op2 }            // Test equivalence
            0xA => { /* CMP */ op1.wrapping_sub(op2) } // Compare (SUB but don't store)
            0xB => { /* CMN */ op1.wrapping_add(op2) } // Compare negative
            0xC => op1 | op2,                          // ORR
            0xD => op2,                                // MOV (ignore op1, just use op2)
            0xE => op1 & !op2,                         // BIC (bit clear)
            0xF => !op2,                               // MVN (move NOT)
            _ => unreachable!(),
        };

        // Write result to Rd (except for TST, TEQ, CMP, CMN which only set flags)
        let is_test_op = matches!(opcode, 0x8 | 0x9 | 0xA | 0xB);
        if !is_test_op {
            self.registers[rd] = result;
        }

        // Update flags if S bit is set
        if set_flags {
            self.set_flag(CPSR_Z, result == 0);
            self.set_flag(CPSR_N, (result >> 31) & 1 == 1);

            // Carry flag depends on the operation type:
            match opcode {
                // Arithmetic ops: carry from the ALU
                0x2 | 0x3 | 0x6 | 0x7 | 0xA => {
                    // SUB/RSB/SBC/RSC/CMP: carry = no borrow
                    self.set_flag(CPSR_C, op1 >= op2);
                }
                0x4 | 0x5 | 0xB => {
                    // ADD/ADC/CMN: carry = unsigned overflow
                    self.set_flag(CPSR_C, (op1 as u64 + op2 as u64) > 0xFFFF_FFFF);
                }
                // Logical ops: carry from the barrel shifter
                0x0 | 0x1 | 0x8 | 0x9 | 0xC | 0xD | 0xE | 0xF => {
                    self.set_flag(CPSR_C, shift_carry);
                }
                _ => {}
            }

            // Overflow flag for arithmetic ops (signed overflow)
            match opcode {
                0x2 | 0x3 | 0xA => {
                    // SUB/RSB/CMP: overflow if signs differ and result sign != expected
                    let overflow = ((op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                0x4 | 0xB => {
                    // ADD/CMN: overflow if same-sign inputs produce different-sign result
                    let overflow = (!(op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                _ => {} // Logical ops don't affect V
            }
        }
    }

    // --- Branch ---
    // B (branch) and BL (branch with link, i.e., function call)
    //
    // Encoding:
    //   bit 24: L flag (1 = save return address in LR, making it a function call)
    //   bits 23-0: signed offset (in units of 4 bytes, so shift left by 2)
    //
    // The offset is signed, so the CPU can jump forward OR backward.

    fn execute_branch(&mut self, instruction: u32) {
        let link = (instruction >> 24) & 1 == 1;

        // The offset is a 24-bit signed number — we must sign-extend it to 32 bits.
        // "Sign extend" means: if bit 23 is 1 (negative), fill the upper bits with 1s.
        let mut offset = instruction & 0x00FF_FFFF;
        if offset & 0x0080_0000 != 0 {
            offset |= 0xFF00_0000; // Sign extend: fill upper 8 bits with 1s
        }
        // Offset is in units of 4 bytes (words), so shift left by 2
        let offset = (offset << 2) as i32;

        if link {
            // BL: save the return address in LR (so the called function can return)
            self.registers[R_LR] = self.registers[R_PC]; // PC already advanced by 4
        }

        // Jump: add the signed offset to PC
        self.registers[R_PC] = (self.registers[R_PC] as i32).wrapping_add(offset) as u32;
    }

    // --- Single Data Transfer (LDR / STR) ---
    // LDR: load a word from memory into a register
    // STR: store a register's value into memory
    //
    // Encoding:
    //   bit 20: L flag (1 = Load/LDR, 0 = Store/STR)
    //   bit 23: U flag (1 = add offset, 0 = subtract offset)
    //   bit 24: P flag (1 = pre-indexed, 0 = post-indexed)
    //   bits 19-16: Rn (base address register)
    //   bits 15-12: Rd (source/destination register)
    //   bits 11-0: offset (immediate or register)

    fn execute_single_transfer(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let is_load = (instruction >> 20) & 1 == 1;
        let add_offset = (instruction >> 23) & 1 == 1;
        let pre_index = (instruction >> 24) & 1 == 1;
        let is_immediate = (instruction >> 25) & 1 == 0; // Note: inverted from data processing!
        let rn = ((instruction >> 16) & 0xF) as usize;
        let rd = ((instruction >> 12) & 0xF) as usize;

        let offset = if is_immediate {
            instruction & 0xFFF // 12-bit immediate offset
        } else {
            // Register offset with optional shift
            let rm = (instruction & 0xF) as usize;
            let shift_type = (instruction >> 5) & 0x3;
            let shift_amount = (instruction >> 7) & 0x1F;
            let (shifted, _) = self.barrel_shift(self.registers[rm], shift_type, shift_amount);
            shifted
        };

        let base = self.registers[rn];
        let addr = if add_offset {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };

        let effective_addr = if pre_index { addr } else { base };

        if is_load {
            self.registers[rd] = bus.read_word(effective_addr);
        } else {
            let value = self.registers[rd];
            bus.write_word(effective_addr, value);
        }

        // Post-index: update base register after the transfer
        if !pre_index {
            self.registers[rn] = addr;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_starts_at_rom_entry() {
        let cpu = Cpu::new();
        assert_eq!(cpu.registers[R_PC], 0x0800_0000);
    }

    #[test]
    fn cpu_stack_pointer_initialized() {
        let cpu = Cpu::new();
        assert_eq!(cpu.registers[R_SP], 0x0300_7F00);
    }

    #[test]
    fn flags_start_clear() {
        let cpu = Cpu::new();
        assert!(!cpu.flag_z());
        assert!(!cpu.flag_n());
        assert!(!cpu.flag_c());
        assert!(!cpu.flag_v());
    }

    #[test]
    fn can_set_and_clear_flags() {
        let mut cpu = Cpu::new();

        cpu.set_flag(CPSR_Z, true);
        assert!(cpu.flag_z());
        assert!(!cpu.flag_n()); // Other flags untouched

        cpu.set_flag(CPSR_Z, false);
        assert!(!cpu.flag_z());
    }

    // --- Helper to run a small program ---
    // We load instructions into ROM and let the CPU execute them.

    fn run_program(instructions: &[u32]) -> (Cpu, crate::bus::Bus) {
        let mut bus = crate::bus::Bus::new();

        // Convert instructions to little-endian bytes and load as ROM
        let mut rom = Vec::new();
        for &inst in instructions {
            rom.extend_from_slice(&inst.to_le_bytes());
        }
        // Pad with zeros (halt) so the CPU stops
        rom.extend_from_slice(&[0; 16]);
        bus.load_rom(rom);

        let mut cpu = Cpu::new();
        // Run until halt (instruction == 0)
        while cpu.step(&mut bus) {}
        (cpu, bus)
    }

    // --- Fetch-decode-execute tests ---

    #[test]
    fn mov_immediate() {
        // MOV R0, #42
        // Encoding: cond=AL(0xE), I=1, opcode=MOV(0xD), S=0, Rd=R0, imm=42
        // 0xE3A00000 | (42)
        // Breakdown: 1110 00 1 1101 0 0000 0000 000000101010
        //            cond    I opc   S Rn   Rd   rotate  imm8
        let (cpu, _) = run_program(&[0xE3A0_002A]); // MOV R0, #42
        assert_eq!(cpu.registers[0], 42);
    }

    #[test]
    fn add_registers() {
        // MOV R0, #10     -> 0xE3A0000A
        // MOV R1, #20     -> 0xE3A01014
        // ADD R2, R0, R1  -> 0xE0802001
        let (cpu, _) = run_program(&[
            0xE3A0_000A, // MOV R0, #10
            0xE3A0_1014, // MOV R1, #20
            0xE080_2001, // ADD R2, R0, R1
        ]);
        assert_eq!(cpu.registers[0], 10);
        assert_eq!(cpu.registers[1], 20);
        assert_eq!(cpu.registers[2], 30); // 10 + 20
    }

    #[test]
    fn sub_immediate() {
        // MOV R0, #100    -> 0xE3A00064
        // SUB R1, R0, #30 -> 0xE240101E
        let (cpu, _) = run_program(&[
            0xE3A0_0064, // MOV R0, #100
            0xE240_101E, // SUB R1, R0, #30
        ]);
        assert_eq!(cpu.registers[1], 70); // 100 - 30
    }

    #[test]
    fn cmp_sets_zero_flag() {
        // MOV R0, #5      -> 0xE3A00005
        // CMP R0, #5      -> 0xE3500005  (SUBS but don't store — sets Z flag)
        let (cpu, _) = run_program(&[
            0xE3A0_0005, // MOV R0, #5
            0xE350_0005, // CMP R0, #5
        ]);
        assert!(cpu.flag_z());  // 5 - 5 = 0, so Z is set
        assert!(cpu.flag_c());  // No borrow, so C is set
    }

    #[test]
    fn conditional_execution() {
        // MOV R0, #5
        // MOV R1, #0      (will stay 0 if conditional MOV is skipped)
        // CMP R0, #3      (5 != 3, so Z is clear)
        // MOVEQ R1, #99   (MOV if EQ — should be SKIPPED because Z is clear)
        // MOVNE R2, #77   (MOV if NE — should EXECUTE because Z is clear)
        let (cpu, _) = run_program(&[
            0xE3A0_0005, // MOV R0, #5
            0xE3A0_1000, // MOV R1, #0
            0xE350_0003, // CMP R0, #3
            0x03A0_1063, // MOVEQ R1, #99  (cond=0x0=EQ, skipped!)
            0x13A0_204D, // MOVNE R2, #77  (cond=0x1=NE, executes!)
        ]);
        assert_eq!(cpu.registers[1], 0);  // MOVEQ was skipped
        assert_eq!(cpu.registers[2], 77); // MOVNE executed
    }

    #[test]
    fn str_and_ldr() {
        // MOV R0, #0xFF        -> store this value
        // STR R0, [R13, #0]    -> store R0 at address in SP (IWRAM)
        // MOV R0, #0           -> clear R0
        // LDR R1, [R13, #0]    -> load it back into R1
        let (cpu, _) = run_program(&[
            0xE3A0_00FF, // MOV R0, #0xFF
            0xE58D_0000, // STR R0, [R13, #0]
            0xE3A0_0000, // MOV R0, #0
            0xE59D_1000, // LDR R1, [R13, #0]
        ]);
        assert_eq!(cpu.registers[0], 0);    // Cleared
        assert_eq!(cpu.registers[1], 0xFF); // Loaded from memory
    }

    // --- Barrel shifter tests ---

    #[test]
    fn add_with_lsl() {
        // MOV R0, #3            -> R0 = 3
        // MOV R1, #5            -> R1 = 5
        // ADD R2, R0, R1, LSL #2  -> R2 = R0 + (R1 << 2) = 3 + 20 = 23
        //
        // ADD R2, R0, R1, LSL #2 encoding:
        //   cond=AL, 00, I=0, ADD=0100, S=0, Rn=R0, Rd=R2, shift_imm=2, LSL=00, 0, Rm=R1
        //   0xE0802101
        let (cpu, _) = run_program(&[
            0xE3A0_0003, // MOV R0, #3
            0xE3A0_1005, // MOV R1, #5
            0xE080_2101, // ADD R2, R0, R1, LSL #2
        ]);
        assert_eq!(cpu.registers[2], 23); // 3 + (5 << 2) = 3 + 20
    }

    #[test]
    fn mov_with_lsr() {
        // MOV R0, #128          -> R0 = 128
        // MOV R1, R0, LSR #3    -> R1 = 128 >> 3 = 16
        //
        // MOV R1, R0, LSR #3:
        //   cond=AL, 00, I=0, MOV=1101, S=0, Rn=0, Rd=R1, shift_imm=3, LSR=01, 0, Rm=R0
        //   0xE1A011A0
        let (cpu, _) = run_program(&[
            0xE3A0_0080, // MOV R0, #128
            0xE1A0_11A0, // MOV R1, R0, LSR #3
        ]);
        assert_eq!(cpu.registers[1], 16); // 128 >> 3
    }

    #[test]
    fn asr_preserves_sign() {
        // We need a negative number (bit 31 set).
        // MVN R0, #0 -> R0 = 0xFFFFFFFF (-1)
        // MOV R0, R0, LSL #24 -> R0 = 0xFF000000 (a large negative in signed)
        // MOV R1, R0, ASR #24 -> R1 = 0xFFFFFFFF (-1, sign preserved)
        //
        // MVN R0, #0:   0xE3E00000
        // LSL R0 by 24: 0xE1A00C00  (MOV R0, R0, LSL #24)
        // ASR by 24:    0xE1A01C40  (MOV R1, R0, ASR #24)
        let (cpu, _) = run_program(&[
            0xE3E0_0000, // MVN R0, #0      -> R0 = 0xFFFFFFFF
            0xE1A0_0C00, // MOV R0, R0, LSL #24 -> R0 = 0xFF000000
            0xE1A0_1C40, // MOV R1, R0, ASR #24 -> R1 = 0xFFFFFFFF
        ]);
        assert_eq!(cpu.registers[0], 0xFF00_0000);
        assert_eq!(cpu.registers[1], 0xFFFF_FFFF); // Sign bit filled in
    }

    #[test]
    fn multiply_by_shift() {
        // A common ARM trick: multiply by 5 using shifts and add.
        // R0 * 5 = R0 * 4 + R0 = (R0 << 2) + R0
        //
        // MOV R0, #7
        // ADD R1, R0, R0, LSL #2  -> R1 = 7 + (7 << 2) = 7 + 28 = 35
        let (cpu, _) = run_program(&[
            0xE3A0_0007, // MOV R0, #7
            0xE080_1100, // ADD R1, R0, R0, LSL #2
        ]);
        assert_eq!(cpu.registers[1], 35); // 7 * 5
    }

    #[test]
    fn overflow_flag_on_signed_add() {
        // 0x7FFFFFFF + 1 should overflow (max positive + 1 = negative in signed)
        // MOV R0, #0x7FFFFFFF is too big for immediate, so build it:
        // MVN R0, #0      -> 0xFFFFFFFF
        // MOV R0, R0, LSR #1 -> 0x7FFFFFFF
        // ADDS R1, R0, #1  -> overflow!
        let (cpu, _) = run_program(&[
            0xE3E0_0000, // MVN R0, #0          -> 0xFFFFFFFF
            0xE1A0_00A0, // MOV R0, R0, LSR #1  -> 0x7FFFFFFF
            0xE290_1001, // ADDS R1, R0, #1     -> 0x80000000 (overflow!)
        ]);
        assert_eq!(cpu.registers[1], 0x8000_0000);
        assert!(cpu.flag_v());  // Signed overflow occurred
        assert!(cpu.flag_n());  // Result is negative (bit 31 set)
        assert!(!cpu.flag_z()); // Result is not zero
    }
}
