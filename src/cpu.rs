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
pub const CPSR_T: u32 = 1 << 5;  // Thumb state (0=ARM, 1=THUMB)

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

    /// Are we in THUMB mode? (T bit in CPSR)
    pub fn in_thumb_mode(&self) -> bool {
        (self.cpsr & CPSR_T) != 0
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
            let rm_val = self.read_reg(rm);

            let shift_type = (instruction >> 5) & 0x3;
            let shift_by_reg = (instruction >> 4) & 1 == 1;

            let amount = if shift_by_reg {
                // Shift amount from register (Rs), using bottom byte only
                let rs = ((instruction >> 8) & 0xF) as usize;
                self.read_reg(rs) & 0xFF
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

    // --- Pipeline Helpers ---
    // The ARM7TDMI has a 3-stage pipeline: fetch → decode → execute.
    // When an instruction executes, the PC has already advanced 2 steps ahead.
    // So reading PC during execution gives: instruction_address + 8.
    //
    // In our emulator, after fetch we advance PC by 4 (to instruction_address + 4).
    // So whenever an instruction reads R15 as an operand, we need to add 4 more
    // to simulate the real pipeline behavior.

    /// Read a register value, accounting for the pipeline when reading PC.
    /// During execution, PC reads as instruction_address + 8 (ARM) or +4 (THUMB).
    fn read_reg(&self, reg: usize) -> u32 {
        if reg == R_PC {
            // PC is currently instruction_addr + step_size (from step's advance).
            // ARM: real hardware sees instruction_addr + 8, we have +4, add 4 more.
            // THUMB: real hardware sees instruction_addr + 4, we have +2, add 2 more.
            if self.in_thumb_mode() {
                self.registers[R_PC].wrapping_add(2)
            } else {
                self.registers[R_PC].wrapping_add(4)
            }
        } else {
            self.registers[reg]
        }
    }

    // --- Fetch-Decode-Execute ---

    /// Execute one CPU step: fetch an instruction, decode it, execute it.
    /// Returns true if execution should continue, false to halt.
    pub fn step(&mut self, bus: &mut crate::bus::Bus) -> bool {
        if self.in_thumb_mode() {
            return self.step_thumb(bus);
        }

        // FETCH: read 32-bit instruction at the PC
        let pc = self.registers[R_PC];
        let instruction = bus.read_word(pc);

        // Advance PC to next instruction (4 bytes ahead for ARM mode).
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
        let bits_27_26 = (instruction >> 26) & 0b11;
        let bit_25 = (instruction >> 25) & 1;

        match bits_27_26 {
            0b00 => {
                // Check for special instructions encoded in the data processing space
                if self.try_special_arm(instruction, bus) {
                    return true;
                }
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

    // --- Special ARM instructions ---
    // Some instructions are encoded in the data processing space (bits 27-26 = 00)
    // but are NOT data processing. We detect them by specific bit patterns.

    /// Try to handle BX, MSR, MRS, or MUL. Returns true if handled.
    fn try_special_arm(&mut self, instruction: u32, _bus: &mut crate::bus::Bus) -> bool {
        // BX — Branch and Exchange (switch ARM <-> THUMB)
        // Pattern: xxxx 0001 0010 1111 1111 1111 0001 xxxx
        // Bits 27-4: 0001_0010_1111_1111_1111_0001
        if instruction & 0x0FFF_FFF0 == 0x012F_FF10 {
            let rm = (instruction & 0xF) as usize;
            let addr = self.read_reg(rm);
            // Bit 0 of the target address determines the new mode:
            //   1 = switch to THUMB, 0 = stay in ARM
            if addr & 1 != 0 {
                self.cpsr |= CPSR_T;  // Enter THUMB mode
                self.registers[R_PC] = addr & !1; // Clear bit 0 for alignment
            } else {
                self.cpsr &= !CPSR_T; // Enter ARM mode
                self.registers[R_PC] = addr & !3; // Word-align
            }
            return true;
        }

        // MSR — Move to Status Register (write to CPSR/SPSR)
        // Pattern: xxxx 00x1 0x10 xxxx 1111 xxxx xxxx xxxx
        if instruction & 0x0DB0_F000 == 0x0120_F000 {
            let use_immediate = (instruction >> 25) & 1 == 1;
            let value = if use_immediate {
                let imm = instruction & 0xFF;
                let rotate = (instruction >> 8) & 0xF;
                imm.rotate_right(rotate * 2)
            } else {
                let rm = (instruction & 0xF) as usize;
                self.read_reg(rm)
            };

            // Which fields to write (bits 19-16 = field mask)
            let field_mask = (instruction >> 16) & 0xF;
            let mut mask = 0u32;
            if field_mask & 0x8 != 0 { mask |= 0xFF00_0000; } // Flags field (N,Z,C,V)
            if field_mask & 0x1 != 0 { mask |= 0x0000_00FF; } // Control field (mode, T, etc.)

            self.cpsr = (self.cpsr & !mask) | (value & mask);
            return true;
        }

        // MRS — Move from Status Register (read CPSR/SPSR)
        // Pattern: xxxx 0001 0x00 1111 xxxx 0000 0000 0000
        if instruction & 0x0FBF_0FFF == 0x010F_0000 {
            let rd = ((instruction >> 12) & 0xF) as usize;
            self.registers[rd] = self.cpsr;
            return true;
        }

        // MUL / MLA — Multiply / Multiply-Accumulate
        // Pattern: xxxx 0000 00xx xxxx xxxx xxxx 1001 xxxx
        if instruction & 0x0FC0_00F0 == 0x0000_0090 {
            let accumulate = (instruction >> 21) & 1 == 1;
            let set_flags = (instruction >> 20) & 1 == 1;
            let rd = ((instruction >> 16) & 0xF) as usize;
            let rn = ((instruction >> 12) & 0xF) as usize;
            let rs = ((instruction >> 8) & 0xF) as usize;
            let rm = (instruction & 0xF) as usize;

            let result = if accumulate {
                // MLA: Rd = Rm * Rs + Rn
                self.registers[rm].wrapping_mul(self.registers[rs]).wrapping_add(self.registers[rn])
            } else {
                // MUL: Rd = Rm * Rs
                self.registers[rm].wrapping_mul(self.registers[rs])
            };
            self.registers[rd] = result;

            if set_flags {
                self.set_flag(CPSR_Z, result == 0);
                self.set_flag(CPSR_N, (result >> 31) & 1 == 1);
            }
            return true;
        }

        // MULL / MLAL — Long Multiply (64-bit result)
        // Pattern: xxxx 0000 1xxx xxxx xxxx xxxx 1001 xxxx
        if instruction & 0x0F80_00F0 == 0x0080_0090 {
            let is_signed = (instruction >> 22) & 1 == 1;
            let accumulate = (instruction >> 21) & 1 == 1;
            let set_flags = (instruction >> 20) & 1 == 1;
            let rd_hi = ((instruction >> 16) & 0xF) as usize;
            let rd_lo = ((instruction >> 12) & 0xF) as usize;
            let rs = ((instruction >> 8) & 0xF) as usize;
            let rm = (instruction & 0xF) as usize;

            let result: u64 = if is_signed {
                (self.registers[rm] as i32 as i64).wrapping_mul(self.registers[rs] as i32 as i64) as u64
            } else {
                (self.registers[rm] as u64).wrapping_mul(self.registers[rs] as u64)
            };

            let result = if accumulate {
                let acc = ((self.registers[rd_hi] as u64) << 32) | (self.registers[rd_lo] as u64);
                result.wrapping_add(acc)
            } else {
                result
            };

            self.registers[rd_lo] = result as u32;
            self.registers[rd_hi] = (result >> 32) as u32;

            if set_flags {
                self.set_flag(CPSR_Z, result == 0);
                self.set_flag(CPSR_N, (result >> 63) & 1 == 1);
            }
            return true;
        }

        false // Not a special instruction
    }

    // ========================================================================
    // THUMB MODE
    // ========================================================================
    // THUMB instructions are 16 bits wide. They're a compressed form of ARM —
    // fewer registers directly accessible (mostly R0-R7), simpler encodings,
    // but the same ALU. Most GBA game code runs in THUMB mode.
    //
    // The CPU switches to THUMB via BX with bit 0 set in the target address.
    // THUMB has ~19 instruction formats, identified by the top bits.

    fn step_thumb(&mut self, bus: &mut crate::bus::Bus) -> bool {
        let pc = self.registers[R_PC];
        let instruction = bus.read_halfword(pc) as u32;

        // Advance PC by 2 (THUMB instructions are 16 bits = 2 bytes)
        self.registers[R_PC] = pc.wrapping_add(2);

        if instruction == 0 {
            return false; // Halt convention
        }

        // Decode by top bits — THUMB uses the top 3-6 bits to identify format
        let top8 = (instruction >> 8) & 0xFF;
        let top5 = (instruction >> 11) & 0x1F;
        let top3 = (instruction >> 13) & 0x7;

        match top3 {
            0b000 => {
                if top5 == 0b00011 {
                    // Format 2: Add/Subtract (register or immediate)
                    self.thumb_add_sub(instruction);
                } else {
                    // Format 1: Move shifted register (LSL, LSR, ASR)
                    self.thumb_shift(instruction);
                }
            }
            0b001 => {
                // Format 3: Immediate operations (MOV, CMP, ADD, SUB with 8-bit immediate)
                self.thumb_immediate(instruction);
            }
            0b010 => {
                if top5 == 0b01000 {
                    if (instruction >> 10) & 1 == 1 {
                        // Format 5: Hi register operations / BX
                        self.thumb_hi_reg_bx(instruction, bus);
                    } else {
                        // Format 4: ALU operations (16 ops between low registers)
                        self.thumb_alu(instruction);
                    }
                } else if top5 == 0b01001 {
                    // Format 6: PC-relative load (LDR Rd, [PC, #imm])
                    self.thumb_pc_relative_load(instruction, bus);
                } else {
                    // Format 7/8: Load/store with register offset
                    self.thumb_load_store_reg(instruction, bus);
                }
            }
            0b011 => {
                // Format 9: Load/store with immediate offset
                self.thumb_load_store_imm(instruction, bus);
            }
            0b100 => {
                if top5 & 0x1E == 0b10000 {
                    // Format 10: Load/store halfword
                    self.thumb_load_store_halfword(instruction, bus);
                } else {
                    // Format 11: SP-relative load/store
                    self.thumb_sp_relative(instruction, bus);
                }
            }
            0b101 => {
                if top5 & 0x1E == 0b10100 {
                    // Format 12: Load address (from PC or SP)
                    self.thumb_load_address(instruction);
                } else if top8 == 0b10110000 {
                    // Format 13: Add offset to SP
                    self.thumb_sp_offset(instruction);
                } else if top5 & 0x1E == 0b10110 {
                    // Format 14: Push/Pop registers
                    self.thumb_push_pop(instruction, bus);
                } else {
                    // Unknown
                }
            }
            0b110 => {
                if top5 & 0x1E == 0b11000 {
                    // Format 15: Multiple load/store (LDMIA/STMIA)
                    self.thumb_multiple_load_store(instruction, bus);
                } else if top5 == 0b11011 || top5 == 0b11010 {
                    // Format 16: Conditional branch
                    self.thumb_conditional_branch(instruction);
                } else if top8 == 0b11011111 {
                    // Format 17: SWI — later
                }
            }
            0b111 => {
                if top5 == 0b11100 {
                    // Format 18: Unconditional branch (B)
                    self.thumb_branch(instruction);
                } else if top5 & 0x1E == 0b11110 {
                    // Format 19: Long branch with link (BL, two-instruction sequence)
                    self.thumb_long_branch(instruction);
                }
            }
            _ => {}
        }

        true
    }

    // --- THUMB Format 1: Move shifted register ---
    // LSL Rd, Rs, #Offset5 / LSR Rd, Rs, #Offset5 / ASR Rd, Rs, #Offset5
    fn thumb_shift(&mut self, instruction: u32) {
        let op = (instruction >> 11) & 0x3;
        let offset = (instruction >> 6) & 0x1F;
        let rs = ((instruction >> 3) & 0x7) as usize;
        let rd = (instruction & 0x7) as usize;

        let value = self.registers[rs];
        // For LSR/ASR, shift of 0 means shift by 32
        let (result, carry) = match op {
            0 => self.barrel_shift(value, 0, offset),       // LSL
            1 => {
                let amt = if offset == 0 { 32 } else { offset };
                self.barrel_shift(value, 1, amt)             // LSR
            }
            2 => {
                let amt = if offset == 0 { 32 } else { offset };
                self.barrel_shift(value, 2, amt)             // ASR
            }
            _ => (value, self.flag_c()),
        };

        self.registers[rd] = result;
        self.set_flag(CPSR_N, (result >> 31) & 1 == 1);
        self.set_flag(CPSR_Z, result == 0);
        self.set_flag(CPSR_C, carry);
    }

    // --- THUMB Format 2: Add/Subtract ---
    // ADD Rd, Rs, Rn/imm3 / SUB Rd, Rs, Rn/imm3
    fn thumb_add_sub(&mut self, instruction: u32) {
        let is_immediate = (instruction >> 10) & 1 == 1;
        let is_sub = (instruction >> 9) & 1 == 1;
        let rn_or_imm = ((instruction >> 6) & 0x7) as u32;
        let rs = ((instruction >> 3) & 0x7) as usize;
        let rd = (instruction & 0x7) as usize;

        let op1 = self.registers[rs];
        let op2 = if is_immediate { rn_or_imm } else { self.registers[rn_or_imm as usize] };

        let result = if is_sub {
            op1.wrapping_sub(op2)
        } else {
            op1.wrapping_add(op2)
        };

        self.registers[rd] = result;
        self.set_flag(CPSR_N, (result >> 31) & 1 == 1);
        self.set_flag(CPSR_Z, result == 0);
        if is_sub {
            self.set_flag(CPSR_C, op1 >= op2);
            self.set_flag(CPSR_V, ((op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0);
        } else {
            self.set_flag(CPSR_C, (op1 as u64 + op2 as u64) > 0xFFFF_FFFF);
            self.set_flag(CPSR_V, (!(op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0);
        }
    }

    // --- THUMB Format 3: Immediate operations ---
    // MOV Rd, #imm8 / CMP Rd, #imm8 / ADD Rd, #imm8 / SUB Rd, #imm8
    fn thumb_immediate(&mut self, instruction: u32) {
        let op = (instruction >> 11) & 0x3;
        let rd = ((instruction >> 8) & 0x7) as usize;
        let imm = instruction & 0xFF;

        let rd_val = self.registers[rd];
        let result = match op {
            0 => imm,                           // MOV
            1 => rd_val.wrapping_sub(imm),      // CMP
            2 => rd_val.wrapping_add(imm),      // ADD
            3 => rd_val.wrapping_sub(imm),      // SUB
            _ => unreachable!(),
        };

        if op != 1 { // CMP doesn't store
            self.registers[rd] = result;
        }

        self.set_flag(CPSR_N, (result >> 31) & 1 == 1);
        self.set_flag(CPSR_Z, result == 0);
        match op {
            1 | 3 => { // CMP, SUB
                self.set_flag(CPSR_C, rd_val >= imm);
                self.set_flag(CPSR_V, ((rd_val ^ imm) & (rd_val ^ result) & 0x8000_0000) != 0);
            }
            2 => { // ADD
                self.set_flag(CPSR_C, (rd_val as u64 + imm as u64) > 0xFFFF_FFFF);
                self.set_flag(CPSR_V, (!(rd_val ^ imm) & (rd_val ^ result) & 0x8000_0000) != 0);
            }
            _ => {} // MOV doesn't change C/V
        }
    }

    // --- THUMB Format 4: ALU operations ---
    // 16 operations on low registers (R0-R7)
    fn thumb_alu(&mut self, instruction: u32) {
        let op = (instruction >> 6) & 0xF;
        let rs = ((instruction >> 3) & 0x7) as usize;
        let rd = (instruction & 0x7) as usize;

        let a = self.registers[rd];
        let b = self.registers[rs];

        let result = match op {
            0x0 => a & b,                                         // AND
            0x1 => a ^ b,                                         // EOR
            0x2 => { let (r, c) = self.barrel_shift(a, 0, b & 0xFF); self.set_flag(CPSR_C, c); r } // LSL
            0x3 => { let (r, c) = self.barrel_shift(a, 1, b & 0xFF); self.set_flag(CPSR_C, c); r } // LSR
            0x4 => { let (r, c) = self.barrel_shift(a, 2, b & 0xFF); self.set_flag(CPSR_C, c); r } // ASR
            0x5 => { // ADC
                let c = if self.flag_c() { 1u32 } else { 0 };
                let r = a.wrapping_add(b).wrapping_add(c);
                self.set_flag(CPSR_C, (a as u64 + b as u64 + c as u64) > 0xFFFF_FFFF);
                self.set_flag(CPSR_V, (!(a ^ b) & (a ^ r) & 0x8000_0000) != 0);
                r
            }
            0x6 => { // SBC
                let c = if self.flag_c() { 1u32 } else { 0 };
                let r = a.wrapping_sub(b).wrapping_add(c).wrapping_sub(1);
                self.set_flag(CPSR_C, (a as u64) >= (b as u64 + 1 - c as u64));
                self.set_flag(CPSR_V, ((a ^ b) & (a ^ r) & 0x8000_0000) != 0);
                r
            }
            0x7 => { let (r, c) = self.barrel_shift(a, 3, b & 0xFF); self.set_flag(CPSR_C, c); r } // ROR
            0x8 => { a & b } // TST (don't store)
            0x9 => { // NEG: Rd = 0 - Rs
                let r = 0u32.wrapping_sub(b);
                self.set_flag(CPSR_C, b == 0);
                self.set_flag(CPSR_V, (b & r & 0x8000_0000) != 0);
                r
            }
            0xA => { // CMP
                let r = a.wrapping_sub(b);
                self.set_flag(CPSR_C, a >= b);
                self.set_flag(CPSR_V, ((a ^ b) & (a ^ r) & 0x8000_0000) != 0);
                r
            }
            0xB => { // CMN
                let r = a.wrapping_add(b);
                self.set_flag(CPSR_C, (a as u64 + b as u64) > 0xFFFF_FFFF);
                self.set_flag(CPSR_V, (!(a ^ b) & (a ^ r) & 0x8000_0000) != 0);
                r
            }
            0xC => a | b,                                          // ORR
            0xD => a.wrapping_mul(b),                              // MUL
            0xE => a & !b,                                         // BIC
            0xF => !b,                                             // MVN
            _ => unreachable!(),
        };

        let is_test = matches!(op, 0x8 | 0xA | 0xB); // TST, CMP, CMN don't store
        if !is_test {
            self.registers[rd] = result;
        }
        self.set_flag(CPSR_N, (result >> 31) & 1 == 1);
        self.set_flag(CPSR_Z, result == 0);
    }

    // --- THUMB Format 5: Hi register operations / BX ---
    // Can access R8-R15! ADD, CMP, MOV between any registers, or BX.
    fn thumb_hi_reg_bx(&mut self, instruction: u32, _bus: &mut crate::bus::Bus) {
        let op = (instruction >> 8) & 0x3;
        let h1 = ((instruction >> 7) & 1) as usize; // High bit for Rd
        let h2 = ((instruction >> 6) & 1) as usize; // High bit for Rs
        let rs = (((instruction >> 3) & 0x7) as usize) | (h2 << 3);
        let rd = ((instruction & 0x7) as usize) | (h1 << 3);

        let rs_val = self.read_reg(rs);

        match op {
            0 => { // ADD
                let rd_val = self.read_reg(rd);
                self.registers[rd] = rd_val.wrapping_add(rs_val);
            }
            1 => { // CMP
                let rd_val = self.read_reg(rd);
                let result = rd_val.wrapping_sub(rs_val);
                self.set_flag(CPSR_N, (result >> 31) & 1 == 1);
                self.set_flag(CPSR_Z, result == 0);
                self.set_flag(CPSR_C, rd_val >= rs_val);
                self.set_flag(CPSR_V, ((rd_val ^ rs_val) & (rd_val ^ result) & 0x8000_0000) != 0);
            }
            2 => { // MOV
                self.registers[rd] = rs_val;
            }
            3 => { // BX
                if rs_val & 1 != 0 {
                    self.cpsr |= CPSR_T;
                    self.registers[R_PC] = rs_val & !1;
                } else {
                    self.cpsr &= !CPSR_T;
                    self.registers[R_PC] = rs_val & !3;
                }
            }
            _ => unreachable!(),
        }
    }

    // --- THUMB Format 6: PC-relative load ---
    // LDR Rd, [PC, #imm8*4]
    fn thumb_pc_relative_load(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let rd = ((instruction >> 8) & 0x7) as usize;
        let offset = (instruction & 0xFF) << 2;
        // PC is word-aligned for this instruction
        let base = self.read_reg(R_PC) & !3;
        self.registers[rd] = bus.read_word(base.wrapping_add(offset));
    }

    // --- THUMB Format 7/8: Load/store with register offset ---
    fn thumb_load_store_reg(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let ro = ((instruction >> 6) & 0x7) as usize;
        let rb = ((instruction >> 3) & 0x7) as usize;
        let rd = (instruction & 0x7) as usize;
        let addr = self.registers[rb].wrapping_add(self.registers[ro]);

        let is_format8 = (instruction >> 9) & 1 == 1;

        if is_format8 {
            // Format 8: halfword/sign-extended
            let op = (instruction >> 10) & 0x3;
            match op {
                0 => bus.write_halfword(addr, self.registers[rd] as u16),   // STRH
                1 => self.registers[rd] = bus.read_byte(addr) as i8 as i32 as u32,  // LDSB
                2 => self.registers[rd] = bus.read_halfword(addr) as u32,   // LDRH
                3 => self.registers[rd] = bus.read_halfword(addr) as i16 as i32 as u32, // LDSH
                _ => unreachable!(),
            }
        } else {
            // Format 7: byte/word
            let is_load = (instruction >> 11) & 1 == 1;
            let is_byte = (instruction >> 10) & 1 == 1;
            if is_load {
                self.registers[rd] = if is_byte {
                    bus.read_byte(addr) as u32
                } else {
                    bus.read_word(addr)
                };
            } else {
                if is_byte {
                    bus.write_byte(addr, self.registers[rd] as u8);
                } else {
                    bus.write_word(addr, self.registers[rd]);
                }
            }
        }
    }

    // --- THUMB Format 9: Load/store with immediate offset ---
    fn thumb_load_store_imm(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let is_load = (instruction >> 11) & 1 == 1;
        let is_byte = (instruction >> 12) & 1 == 1;
        let offset = (instruction >> 6) & 0x1F;
        let rb = ((instruction >> 3) & 0x7) as usize;
        let rd = (instruction & 0x7) as usize;

        let offset = if is_byte { offset } else { offset << 2 }; // Word offset * 4
        let addr = self.registers[rb].wrapping_add(offset);

        if is_load {
            self.registers[rd] = if is_byte {
                bus.read_byte(addr) as u32
            } else {
                bus.read_word(addr)
            };
        } else {
            if is_byte {
                bus.write_byte(addr, self.registers[rd] as u8);
            } else {
                bus.write_word(addr, self.registers[rd]);
            }
        }
    }

    // --- THUMB Format 10: Load/store halfword ---
    fn thumb_load_store_halfword(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let is_load = (instruction >> 11) & 1 == 1;
        let offset = ((instruction >> 6) & 0x1F) << 1; // Halfword offset * 2
        let rb = ((instruction >> 3) & 0x7) as usize;
        let rd = (instruction & 0x7) as usize;
        let addr = self.registers[rb].wrapping_add(offset);

        if is_load {
            self.registers[rd] = bus.read_halfword(addr) as u32;
        } else {
            bus.write_halfword(addr, self.registers[rd] as u16);
        }
    }

    // --- THUMB Format 11: SP-relative load/store ---
    fn thumb_sp_relative(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let is_load = (instruction >> 11) & 1 == 1;
        let rd = ((instruction >> 8) & 0x7) as usize;
        let offset = (instruction & 0xFF) << 2;
        let addr = self.registers[R_SP].wrapping_add(offset);

        if is_load {
            self.registers[rd] = bus.read_word(addr);
        } else {
            bus.write_word(addr, self.registers[rd]);
        }
    }

    // --- THUMB Format 12: Load address ---
    // ADD Rd, PC/SP, #imm8*4
    fn thumb_load_address(&mut self, instruction: u32) {
        let is_sp = (instruction >> 11) & 1 == 1;
        let rd = ((instruction >> 8) & 0x7) as usize;
        let offset = (instruction & 0xFF) << 2;

        let base = if is_sp {
            self.registers[R_SP]
        } else {
            self.read_reg(R_PC) & !3 // PC, word-aligned
        };
        self.registers[rd] = base.wrapping_add(offset);
    }

    // --- THUMB Format 13: Add offset to SP ---
    fn thumb_sp_offset(&mut self, instruction: u32) {
        let offset = (instruction & 0x7F) << 2;
        if (instruction >> 7) & 1 == 1 {
            self.registers[R_SP] = self.registers[R_SP].wrapping_sub(offset);
        } else {
            self.registers[R_SP] = self.registers[R_SP].wrapping_add(offset);
        }
    }

    // --- THUMB Format 14: Push/Pop ---
    fn thumb_push_pop(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let is_pop = (instruction >> 11) & 1 == 1;
        let store_lr_load_pc = (instruction >> 8) & 1 == 1;
        let reg_list = instruction & 0xFF;

        if is_pop {
            // POP {Rlist} — load registers from stack
            let mut addr = self.registers[R_SP];
            for i in 0..8 {
                if reg_list & (1 << i) != 0 {
                    self.registers[i] = bus.read_word(addr);
                    addr = addr.wrapping_add(4);
                }
            }
            if store_lr_load_pc {
                let val = bus.read_word(addr);
                // POP {PC} can switch to ARM mode if bit 0 is clear
                if val & 1 != 0 {
                    self.registers[R_PC] = val & !1;
                } else {
                    self.cpsr &= !CPSR_T;
                    self.registers[R_PC] = val & !3;
                }
                addr = addr.wrapping_add(4);
            }
            self.registers[R_SP] = addr;
        } else {
            // PUSH {Rlist} — store registers to stack
            let mut count = 0u32;
            for i in 0..8 {
                if reg_list & (1 << i) != 0 { count += 1; }
            }
            if store_lr_load_pc { count += 1; }

            let mut addr = self.registers[R_SP].wrapping_sub(count * 4);
            self.registers[R_SP] = addr;

            for i in 0..8 {
                if reg_list & (1 << i) != 0 {
                    bus.write_word(addr, self.registers[i]);
                    addr = addr.wrapping_add(4);
                }
            }
            if store_lr_load_pc {
                bus.write_word(addr, self.registers[R_LR]);
            }
        }
    }

    // --- THUMB Format 15: Multiple load/store ---
    fn thumb_multiple_load_store(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let is_load = (instruction >> 11) & 1 == 1;
        let rb = ((instruction >> 8) & 0x7) as usize;
        let reg_list = instruction & 0xFF;
        let mut addr = self.registers[rb];

        for i in 0..8 {
            if reg_list & (1 << i) != 0 {
                if is_load {
                    self.registers[i] = bus.read_word(addr);
                } else {
                    bus.write_word(addr, self.registers[i]);
                }
                addr = addr.wrapping_add(4);
            }
        }
        self.registers[rb] = addr; // Write-back
    }

    // --- THUMB Format 16: Conditional branch ---
    fn thumb_conditional_branch(&mut self, instruction: u32) {
        let cond = (instruction >> 8) & 0xF;
        if !self.condition_met(cond) {
            return;
        }
        // 8-bit signed offset, in units of 2 bytes
        let mut offset = instruction & 0xFF;
        if offset & 0x80 != 0 {
            offset |= 0xFFFF_FF00; // Sign extend
        }
        let offset = (offset << 1) as i32;
        let pc = self.read_reg(R_PC); // PC + 4 in THUMB
        self.registers[R_PC] = (pc as i32).wrapping_add(offset) as u32;
    }

    // --- THUMB Format 18: Unconditional branch ---
    fn thumb_branch(&mut self, instruction: u32) {
        let mut offset = instruction & 0x7FF;
        if offset & 0x400 != 0 {
            offset |= 0xFFFF_F800; // Sign extend 11-bit
        }
        let offset = (offset << 1) as i32;
        let pc = self.read_reg(R_PC);
        self.registers[R_PC] = (pc as i32).wrapping_add(offset) as u32;
    }

    // --- THUMB Format 19: Long branch with link (BL) ---
    // This is a two-instruction sequence:
    //   Instruction 1 (H=0): LR = PC + (offset_high << 12)
    //   Instruction 2 (H=1): PC = LR + (offset_low << 1), LR = old_PC | 1
    fn thumb_long_branch(&mut self, instruction: u32) {
        let h = (instruction >> 11) & 1;
        let offset = instruction & 0x7FF;

        if h == 0 {
            // First instruction: set up LR with high part of offset
            let mut off = offset << 12;
            if off & 0x0040_0000 != 0 {
                off |= 0xFF80_0000; // Sign extend 23-bit
            }
            let pc = self.read_reg(R_PC);
            self.registers[R_LR] = pc.wrapping_add(off);
        } else {
            // Second instruction: jump and link
            let target = self.registers[R_LR].wrapping_add(offset << 1);
            self.registers[R_LR] = (self.registers[R_PC].wrapping_sub(2)) | 1; // Return addr with THUMB bit
            self.registers[R_PC] = target & !1;
        }
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

        let op1 = self.read_reg(rn);

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
            self.registers[R_LR] = self.registers[R_PC]; // PC = instruction_addr + 4
        }

        // Jump: target = PC(+8) + offset
        // read_reg(R_PC) gives instruction_addr + 8 (pipeline-correct)
        let pc = self.read_reg(R_PC);
        self.registers[R_PC] = (pc as i32).wrapping_add(offset) as u32;
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
            let (shifted, _) = self.barrel_shift(self.read_reg(rm), shift_type, shift_amount);
            shifted
        };

        let base = self.read_reg(rn);
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

    #[test]
    fn loop_sum_1_to_10() {
        // Full integration test: loop, branch, compare, memory store
        // Branch offsets now use correct pipeline model: target = PC+8 + offset*4
        let (cpu, bus) = run_program(&[
            0xE3A0_0000, // 0x00: MOV R0, #0       ; counter = 0
            0xE3A0_100A, // 0x04: MOV R1, #10      ; limit = 10
            0xE3A0_2000, // 0x08: MOV R2, #0       ; sum = 0
            0xE280_0001, // 0x0C: ADD R0, R0, #1   ; counter++        ← loop
            0xE082_2000, // 0x10: ADD R2, R2, R0   ; sum += counter
            0xE150_0001, // 0x14: CMP R0, R1       ; counter == limit?
            0x1AFF_FFFB, // 0x18: BNE loop         ; offset=-5: (0x0C - 0x18 - 8)/4
            0xE3A0_3003, // 0x1C: MOV R3, #3
            0xE1A0_3C03, // 0x20: MOV R3, R3, LSL #24  ; R3 = 0x03000000
            0xE583_2000, // 0x24: STR R2, [R3]     ; IWRAM[0] = 55
        ]);
        assert_eq!(cpu.registers[0], 10);
        assert_eq!(cpu.registers[2], 55);                  // 1+2+...+10
        assert_eq!(cpu.registers[3], 0x0300_0000);         // IWRAM base
        assert_eq!(bus.read_word(0x0300_0000), 55);        // Stored in memory!
    }
}
