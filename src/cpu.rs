/// The ARM7TDMI CPU — the brain of the GBA.
///
/// The real chip runs at 16.78 MHz (about 16 million instructions per second).
/// We model its state as registers + a reference to memory.

/// ARM7TDMI processor modes, identified by bits 4-0 of CPSR.
///
/// Each mode has its own banked copies of certain registers:
/// - All modes bank R13 (SP) and R14 (LR)
/// - FIQ additionally banks R8-R12 (for fast interrupt response)
/// - Each non-User/System mode has its own SPSR (Saved Program Status Register)
///
/// Mode bits:
///   0x10 = User       — normal program execution
///   0x11 = FIQ        — fast interrupt (extra banked regs for speed)
///   0x12 = IRQ        — normal interrupt
///   0x13 = Supervisor — entered via SWI (BIOS calls)
///   0x17 = Abort      — memory access violation
///   0x1B = Undefined  — undefined instruction trap
///   0x1F = System     — privileged User mode (shares User's banked regs)

/// Index into banked register arrays. We store one set per privileged mode.
/// User and System share the same bank (index 0).
const BANK_USR: usize = 0; // User/System
const BANK_FIQ: usize = 1;
const BANK_IRQ: usize = 2;
const BANK_SVC: usize = 3; // Supervisor
const BANK_ABT: usize = 4; // Abort
const BANK_UND: usize = 5; // Undefined
const NUM_BANKS: usize = 6;

/// Map CPSR mode bits to a bank index
fn mode_to_bank(mode_bits: u32) -> usize {
    match mode_bits & 0x1F {
        0x10 | 0x1F => BANK_USR, // User and System share banks
        0x11 => BANK_FIQ,
        0x12 => BANK_IRQ,
        0x13 => BANK_SVC,
        0x17 => BANK_ABT,
        0x1B => BANK_UND,
        _ => BANK_USR, // Fallback for invalid mode bits
    }
}

/// The CPU state: 16 registers + status register + banked registers.
///
/// The ARM7TDMI has 37 total registers:
///   - 16 "current" registers (R0-R15)
///   - 10 banked copies of R13/R14 (one pair per non-User mode)
///   - 5 banked R8-R12 for FIQ mode
///   - 5 SPSRs (one per non-User/System mode)
///   - 1 CPSR
pub struct Cpu {
    /// R0 through R15. Index 15 is the Program Counter (PC).
    /// R13 and R14 are swapped in/out when the CPU changes mode.
    pub registers: [u32; 16],

    /// Current Program Status Register.
    /// Bit 31: N (Negative)  Bit 30: Z (Zero)
    /// Bit 29: C (Carry)     Bit 28: V (Overflow)
    /// Bit 5: T (Thumb)      Bits 4-0: Mode
    pub cpsr: u32,

    /// Banked R13 (SP) for each mode — swapped when mode changes
    banked_sp: [u32; NUM_BANKS],

    /// Banked R14 (LR) for each mode — swapped when mode changes
    banked_lr: [u32; NUM_BANKS],

    /// Banked R8-R12 for FIQ mode (all other modes share the main set)
    banked_fiq_r8_r12: [u32; 5],

    /// Saved copies of R8-R12 from non-FIQ mode (restored when leaving FIQ)
    banked_usr_r8_r12: [u32; 5],

    /// Saved Program Status Registers — one per privileged mode.
    /// When an exception occurs, the current CPSR is saved to the new mode's SPSR.
    /// SPSR[0] (User/System) is unused since those modes can't have a saved status.
    pub spsr: [u32; NUM_BANKS],

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
            banked_sp: [0; NUM_BANKS],
            banked_lr: [0; NUM_BANKS],
            banked_fiq_r8_r12: [0; 5],
            banked_usr_r8_r12: [0; 5],
            spsr: [0; NUM_BANKS],
        };

        // The PC starts at the beginning of ROM
        cpu.registers[R_PC] = 0x0800_0000;

        // The stack pointer is conventionally initialized to the top of IWRAM
        cpu.registers[R_SP] = 0x0300_7F00;
        cpu.banked_sp[BANK_USR] = 0x0300_7F00;

        cpu
    }

    /// Set the banked SP for a given mode (used during BIOS skip initialization).
    pub fn set_mode_sp(&mut self, mode_bits: u32, sp: u32) {
        let bank = mode_to_bank(mode_bits);
        self.banked_sp[bank] = sp;
    }

    /// Switch banked registers when the CPU mode changes.
    ///
    /// On real hardware this happens automatically in the same cycle as the
    /// mode change. We save the current mode's SP/LR into their bank, then
    /// load the new mode's SP/LR from their bank.
    fn switch_mode(&mut self, old_mode: u32, new_mode: u32) {
        let old_bank = mode_to_bank(old_mode);
        let new_bank = mode_to_bank(new_mode);

        if old_bank == new_bank {
            return; // Same bank (e.g., User ↔ System), nothing to swap
        }

        // Save current R13/R14 to old bank
        self.banked_sp[old_bank] = self.registers[R_SP];
        self.banked_lr[old_bank] = self.registers[R_LR];

        // FIQ banks R8-R12 as well
        if old_bank == BANK_FIQ {
            for i in 0..5 {
                self.banked_fiq_r8_r12[i] = self.registers[8 + i];
                self.registers[8 + i] = self.banked_usr_r8_r12[i];
            }
        } else if new_bank == BANK_FIQ {
            for i in 0..5 {
                self.banked_usr_r8_r12[i] = self.registers[8 + i];
                self.registers[8 + i] = self.banked_fiq_r8_r12[i];
            }
        }

        // Load R13/R14 from new bank
        self.registers[R_SP] = self.banked_sp[new_bank];
        self.registers[R_LR] = self.banked_lr[new_bank];
    }

    /// Write to CPSR with automatic bank switching when mode bits change.
    /// `mask` controls which fields are written (e.g., flags only vs full CPSR).
    fn write_cpsr(&mut self, value: u32, mask: u32) {
        let old_mode = self.cpsr & 0x1F;
        self.cpsr = (self.cpsr & !mask) | (value & mask);
        let new_mode = self.cpsr & 0x1F;

        if old_mode != new_mode {
            self.switch_mode(old_mode, new_mode);
        }
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
    /// Barrel shifter — shifts/rotates a value and produces a carry-out.
    ///
    /// `is_immediate_shift` distinguishes between:
    ///   - Immediate shift (5-bit amount encoded in instruction): amount=0 has special meanings
    ///   - Register shift (amount from Rs): amount=0 means "don't shift"
    fn barrel_shift_ext(&self, value: u32, shift_type: u32, amount: u32, is_immediate_shift: bool) -> (u32, bool) {
        let carry_in = self.flag_c();

        // For register shifts, amount=0 always means "no shift, carry unchanged"
        // For immediate shifts, amount=0 has special encodings per shift type
        if amount == 0 {
            if !is_immediate_shift || shift_type == 0 {
                // LSL #0 or register-shift by 0: identity, carry unchanged
                return (value, carry_in);
            }
            // Immediate shift of 0 with non-LSL types has special meaning:
            return match shift_type {
                1 => {
                    // LSR #0 encodes LSR #32: result=0, carry=bit 31
                    (0, (value >> 31) & 1 != 0)
                }
                2 => {
                    // ASR #0 encodes ASR #32: all bits become sign bit
                    let sign = (value as i32) >> 31;
                    (sign as u32, (value >> 31) & 1 != 0)
                }
                3 => {
                    // ROR #0 encodes RRX: rotate right by 1 through carry
                    let carry = value & 1 != 0;
                    let result = (value >> 1) | ((carry_in as u32) << 31);
                    (result, carry)
                }
                _ => unreachable!(),
            };
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
                    let sign = (value as i32) >> 31;
                    (sign as u32, (value >> 31) & 1 != 0)
                } else {
                    let carry = (value >> (amount - 1)) & 1 != 0;
                    ((value as i32 >> amount) as u32, carry)
                }
            }
            3 => { // ROR — Rotate Right
                let amount = amount % 32;
                if amount == 0 {
                    (value, (value >> 31) & 1 != 0)
                } else {
                    let carry = (value >> (amount - 1)) & 1 != 0;
                    (value.rotate_right(amount), carry)
                }
            }
            _ => unreachable!(),
        }
    }

    /// Convenience wrapper used by existing code (assumes immediate shift encoding)
    fn barrel_shift(&self, value: u32, shift_type: u32, amount: u32) -> (u32, bool) {
        self.barrel_shift_ext(value, shift_type, amount, true)
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
            let shift_type = (instruction >> 5) & 0x3;
            let shift_by_reg = (instruction >> 4) & 1 == 1;

            // When operand2 uses a register-specified shift (bit 4=1),
            // the ARM7TDMI takes an extra internal cycle. If Rm is PC,
            // this means PC has advanced one more step: reads as
            // instruction_addr + 12 instead of the usual +8.
            let rm_val = if shift_by_reg && rm == R_PC {
                self.read_reg(rm).wrapping_add(4)
            } else {
                self.read_reg(rm)
            };

            if shift_by_reg {
                // Shift amount from register (Rs), using bottom byte only
                let rs = ((instruction >> 8) & 0xF) as usize;
                let amount = self.read_reg(rs) & 0xFF;
                // Register-specified shift: amount=0 means "no shift"
                self.barrel_shift_ext(rm_val, shift_type, amount, false)
            } else {
                // Shift amount is a 5-bit immediate
                let amount = (instruction >> 7) & 0x1F;
                // Immediate shift: amount=0 has special encodings (LSR#32, ASR#32, RRX)
                self.barrel_shift_ext(rm_val, shift_type, amount, true)
            }
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
    /// Estimate the fetch waitstate cost for an instruction at the given PC.
    ///
    /// The GBA has different memory regions with different access speeds:
    ///   BIOS/IWRAM: 0 extra cycles (fast, on-chip)
    ///   EWRAM:      2 extra cycles (16-bit bus, off-chip)
    ///   ROM (WS0):  2 extra cycles for sequential THUMB, 4 for sequential ARM
    ///               (ROM has a 16-bit bus, so 32-bit ARM fetches need 2 accesses)
    ///
    /// Real hardware uses WAITCNT (0x04000204) to configure these, but most
    /// games use the default settings. This approximation covers the common case.
    fn fetch_waitstates(&self, pc: u32) -> u32 {
        match pc >> 24 {
            0x00 => 0,       // BIOS — no wait
            0x02 => 2,       // EWRAM — 16-bit bus, 2 wait cycles
            0x03 => 0,       // IWRAM — fast, no wait
            0x08..=0x0D => { // ROM (WS0/1/2) — 16-bit bus
                if self.in_thumb_mode() { 2 } else { 4 } // ARM needs 2 sequential accesses
            }
            _ => 0,
        }
    }

    /// Execute one instruction and return the number of cycles consumed.
    /// Returns 0 to signal a halt (instruction was 0x00000000).
    ///
    /// Cycle cost = instruction execution + fetch waitstates.
    /// Execution costs (ARM7TDMI approximations):
    ///   ALU ops: 1 cycle    Branches: 3 cycles
    ///   LDR:     3 cycles   STR:      2 cycles
    ///   LDM/STM: 2+n cycles MUL:      4 cycles
    pub fn step(&mut self, bus: &mut crate::bus::Bus) -> u32 {
        // Check for pending IRQ: CPSR I-bit must be clear (IRQs enabled)
        if self.cpsr & 0x80 == 0 && bus.irq_pending() {
            self.enter_irq();
        }

        if self.in_thumb_mode() {
            return self.step_thumb(bus);
        }

        // FETCH: read 32-bit instruction at the PC
        let pc = self.registers[R_PC];
        let wait = self.fetch_waitstates(pc);
        let instruction = bus.read_word(pc);

        // Advance PC to next instruction (4 bytes ahead for ARM mode).
        self.registers[R_PC] = pc.wrapping_add(4);

        // If instruction is 0, treat as halt (no real instruction is all zeros in practice)
        if instruction == 0 {
            return 0;
        }

        // Check condition code (bits 31-28) — should we even execute this?
        let cond = (instruction >> 28) & 0xF;
        if !self.condition_met(cond) {
            return 1 + wait; // Skip this instruction, but keep running
        }

        // DECODE: determine instruction type from bit patterns
        let bits_27_26 = (instruction >> 26) & 0b11;
        let bit_25 = (instruction >> 25) & 1;

        let exec_cycles = match bits_27_26 {
            0b00 => {
                // Check for special instructions encoded in the data processing space
                if self.try_special_arm(instruction, bus) {
                    let is_bx = (instruction & 0x0FFF_FFF0) == 0x012F_FF10;
                    let is_mul = (instruction & 0x0FC0_00F0) == 0x0000_0090;
                    return (if is_bx { 3 } else if is_mul { 4 } else { 1 }) + wait;
                }
                // Halfword/signed transfer
                if bit_25 == 0 && (instruction & 0x90) == 0x90 && (instruction & 0x0200_0000) == 0 {
                    let sh = (instruction >> 5) & 0x3;
                    if sh != 0 {
                        self.execute_halfword_transfer(instruction, bus);
                        let is_load = (instruction >> 20) & 1 == 1;
                        return (if is_load { 3 } else { 2 }) + wait;
                    }
                }
                // Data processing (ALU operations)
                self.execute_data_processing(instruction, bit_25);
                1
            }
            0b01 => {
                self.execute_single_transfer(instruction, bus);
                let is_load = (instruction >> 20) & 1 == 1;
                if is_load { 3 } else { 2 }
            }
            0b10 => {
                if (instruction >> 25) & 1 == 1 {
                    self.execute_branch(instruction);
                    3
                } else {
                    self.execute_block_transfer(instruction, bus);
                    let reg_count = (instruction & 0xFFFF).count_ones();
                    2 + reg_count
                }
            }
            0b11 => {
                if (instruction >> 24) & 0xF == 0xF {
                    self.execute_swi(instruction, bus);
                    3
                } else {
                    1
                }
            }
            _ => unreachable!(),
        };

        exec_cycles + wait
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

            let is_spsr = (instruction >> 22) & 1 == 1;
            if is_spsr {
                let bank = mode_to_bank(self.cpsr & 0x1F);
                self.spsr[bank] = (self.spsr[bank] & !mask) | (value & mask);
            } else {
                self.write_cpsr(value, mask);
            }
            return true;
        }

        // MRS — Move from Status Register (read CPSR/SPSR)
        // Pattern: xxxx 0001 0x00 1111 xxxx 0000 0000 0000
        if instruction & 0x0FBF_0FFF == 0x010F_0000 {
            let rd = ((instruction >> 12) & 0xF) as usize;
            let is_spsr = (instruction >> 22) & 1 == 1;
            if is_spsr {
                let bank = mode_to_bank(self.cpsr & 0x1F);
                self.registers[rd] = self.spsr[bank];
            } else {
                self.registers[rd] = self.cpsr;
            }
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

    fn step_thumb(&mut self, bus: &mut crate::bus::Bus) -> u32 {
        let pc = self.registers[R_PC];
        let wait = self.fetch_waitstates(pc);
        let instruction = bus.read_halfword(pc) as u32;

        // Advance PC by 2 (THUMB instructions are 16 bits = 2 bytes)
        self.registers[R_PC] = pc.wrapping_add(2);

        if instruction == 0 {
            return 0; // Halt convention
        }

        // Decode by top bits — THUMB uses the top 3-6 bits to identify format
        let top8 = (instruction >> 8) & 0xFF;
        let top5 = (instruction >> 11) & 0x1F;
        let top3 = (instruction >> 13) & 0x7;

        let exec_cycles = match top3 {
            0b000 => {
                if top5 == 0b00011 {
                    self.thumb_add_sub(instruction);
                } else {
                    self.thumb_shift(instruction);
                }
                1
            }
            0b001 => {
                self.thumb_immediate(instruction);
                1
            }
            0b010 => {
                if top5 == 0b01000 {
                    if (instruction >> 10) & 1 == 1 {
                        self.thumb_hi_reg_bx(instruction, bus);
                        let op = (instruction >> 8) & 0x3;
                        if op == 3 { 3 } else { 1 }
                    } else {
                        self.thumb_alu(instruction);
                        1
                    }
                } else if top5 == 0b01001 {
                    self.thumb_pc_relative_load(instruction, bus);
                    3
                } else {
                    self.thumb_load_store_reg(instruction, bus);
                    3
                }
            }
            0b011 => {
                self.thumb_load_store_imm(instruction, bus);
                let is_load = (instruction >> 11) & 1 == 1;
                if is_load { 3 } else { 2 }
            }
            0b100 => {
                if top5 & 0x1E == 0b10000 {
                    self.thumb_load_store_halfword(instruction, bus);
                    let is_load = (instruction >> 11) & 1 == 1;
                    if is_load { 3 } else { 2 }
                } else {
                    self.thumb_sp_relative(instruction, bus);
                    let is_load = (instruction >> 11) & 1 == 1;
                    if is_load { 3 } else { 2 }
                }
            }
            0b101 => {
                if top5 & 0x1E == 0b10100 {
                    self.thumb_load_address(instruction);
                    1
                } else if top8 == 0b10110000 {
                    self.thumb_sp_offset(instruction);
                    1
                } else if top5 & 0x1E == 0b10110 {
                    self.thumb_push_pop(instruction, bus);
                    let reg_count = (instruction & 0xFF).count_ones()
                        + ((instruction >> 8) & 1);
                    2 + reg_count
                } else {
                    1
                }
            }
            0b110 => {
                if top5 & 0x1E == 0b11000 {
                    self.thumb_multiple_load_store(instruction, bus);
                    let reg_count = (instruction & 0xFF).count_ones();
                    2 + reg_count
                } else if top8 == 0b11011111 {
                    self.execute_swi_thumb(instruction, bus);
                    3
                } else if top5 == 0b11011 || top5 == 0b11010 {
                    self.thumb_conditional_branch(instruction);
                    3
                } else {
                    1
                }
            }
            0b111 => {
                if top5 == 0b11100 {
                    self.thumb_branch(instruction);
                    3
                } else if top5 & 0x1E == 0b11110 {
                    self.thumb_long_branch(instruction);
                    1
                } else {
                    1
                }
            }
            _ => { 1 }
        };

        exec_cycles + wait
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
            0x2 => { let (r, c) = self.barrel_shift_ext(a, 0, b & 0xFF, false); self.set_flag(CPSR_C, c); r } // LSL
            0x3 => { let (r, c) = self.barrel_shift_ext(a, 1, b & 0xFF, false); self.set_flag(CPSR_C, c); r } // LSR
            0x4 => { let (r, c) = self.barrel_shift_ext(a, 2, b & 0xFF, false); self.set_flag(CPSR_C, c); r } // ASR
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
            0x7 => { let (r, c) = self.barrel_shift_ext(a, 3, b & 0xFF, false); self.set_flag(CPSR_C, c); r } // ROR
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
            0xD => {                                                  // MUL
                self.set_flag(CPSR_C, false); // C destroyed (set to 0)
                a.wrapping_mul(b)
            }
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
                let result = rd_val.wrapping_add(rs_val);
                if rd == R_PC {
                    self.registers[R_PC] = result & !1;
                } else {
                    self.registers[rd] = result;
                }
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
                if rd == R_PC {
                    // MOV PC, Rs — branch (halfword-align on ARM7TDMI)
                    self.registers[R_PC] = rs_val & !1;
                } else {
                    self.registers[rd] = rs_val;
                }
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
                    // LDR: unaligned → force-align and rotate (same as ARM)
                    let misalign = addr & 3;
                    let aligned = addr & !3;
                    let val = bus.read_word(aligned);
                    if misalign != 0 { val.rotate_right(misalign * 8) } else { val }
                };
            } else {
                if is_byte {
                    bus.write_byte(addr, self.registers[rd] as u8);
                } else {
                    bus.write_word(addr & !3, self.registers[rd]);
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
                // LDR: unaligned → force-align and rotate
                let misalign = addr & 3;
                let aligned = addr & !3;
                let val = bus.read_word(aligned);
                if misalign != 0 { val.rotate_right(misalign * 8) } else { val }
            };
        } else {
            if is_byte {
                bus.write_byte(addr, self.registers[rd] as u8);
            } else {
                bus.write_word(addr & !3, self.registers[rd]);
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
            // LR = address of next instruction (after this 2-byte instruction) with THUMB bit set
            self.registers[R_LR] = self.registers[R_PC] | 1;
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
        if set_flags && rd == R_PC && !is_test_op {
            // Special case: S=1 and Rd=PC means "copy SPSR to CPSR" (exception return)
            let bank = mode_to_bank(self.cpsr & 0x1F);
            self.write_cpsr(self.spsr[bank], 0xFFFF_FFFF);
        } else if set_flags {
            self.set_flag(CPSR_Z, result == 0);
            self.set_flag(CPSR_N, (result >> 31) & 1 == 1);

            // Carry and overflow flags depend on the operation type.
            // For arithmetic ops, we compute the "true" 64-bit or borrow result.
            // For logical ops, carry comes from the barrel shifter, V is unchanged.
            match opcode {
                0x2 | 0xA => {
                    // SUB/CMP: result = op1 - op2
                    // Carry = no borrow = op1 >= op2
                    self.set_flag(CPSR_C, op1 >= op2);
                    let overflow = ((op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                0x3 => {
                    // RSB: result = op2 - op1 (reversed operands)
                    self.set_flag(CPSR_C, op2 >= op1);
                    let overflow = ((op2 ^ op1) & (op2 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                0x4 | 0xB => {
                    // ADD/CMN: carry = unsigned overflow
                    let wide = op1 as u64 + op2 as u64;
                    self.set_flag(CPSR_C, wide > 0xFFFF_FFFF);
                    let overflow = (!(op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                0x5 => {
                    // ADC: result = op1 + op2 + C
                    let c = if self.flag_c() { 1u64 } else { 0 };
                    let wide = op1 as u64 + op2 as u64 + c;
                    self.set_flag(CPSR_C, wide > 0xFFFF_FFFF);
                    let overflow = (!(op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                0x6 => {
                    // SBC: result = op1 - op2 + C - 1 = op1 - op2 - !C
                    let c = if self.flag_c() { 1u64 } else { 0 };
                    let wide = op1 as u64 + (!op2) as u64 + c;
                    self.set_flag(CPSR_C, wide > 0xFFFF_FFFF);
                    // For SBC overflow: treat as op1 - (op2 + !C)
                    let overflow = ((op1 ^ op2) & (op1 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                0x7 => {
                    // RSC: result = op2 - op1 + C - 1 = op2 - op1 - !C
                    let c = if self.flag_c() { 1u64 } else { 0 };
                    let wide = op2 as u64 + (!op1) as u64 + c;
                    self.set_flag(CPSR_C, wide > 0xFFFF_FFFF);
                    let overflow = ((op2 ^ op1) & (op2 ^ result) & 0x8000_0000) != 0;
                    self.set_flag(CPSR_V, overflow);
                }
                // Logical ops: carry from barrel shifter, V unchanged
                0x0 | 0x1 | 0x8 | 0x9 | 0xC | 0xD | 0xE | 0xF => {
                    self.set_flag(CPSR_C, shift_carry);
                }
                _ => {}
            }
        } // end else if set_flags
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
        let write_back = (instruction >> 21) & 1 == 1; // W bit: write-back for pre-indexed
        let is_byte = (instruction >> 22) & 1 == 1; // B bit: 1=byte, 0=word
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
            self.registers[rd] = if is_byte {
                bus.read_byte(effective_addr) as u32
            } else {
                // ARM7TDMI LDR: unaligned address → force-align, then rotate right
                let misalign = effective_addr & 3;
                let aligned = effective_addr & !3;
                let val = bus.read_word(aligned);
                if misalign != 0 {
                    val.rotate_right(misalign * 8)
                } else {
                    val
                }
            };
        } else {
            if is_byte {
                bus.write_byte(effective_addr, self.registers[rd] as u8);
            } else {
                // STR: force-align address
                bus.write_word(effective_addr & !3, self.registers[rd]);
            }
        }

        // Write-back: update base register with the offset address.
        // Post-index always writes back. Pre-index only if W bit is set.
        // For loads: if Rd == Rn, the loaded value takes priority (skip write-back).
        if (!pre_index || write_back) && !(is_load && rd == rn) {
            self.registers[rn] = addr;
        }
    }

    // --- Block Data Transfer (LDM / STM) ---
    // Load/store multiple registers at once. Used for push/pop and fast memory copy.
    //
    // Encoding:
    //   bit 24: P (pre/post indexing)
    //   bit 23: U (up/down — add or subtract offset)
    //   bit 22: S (load PSR or force user mode)
    //   bit 21: W (write-back — update base register)
    //   bit 20: L (load/store)
    //   bits 19-16: Rn (base register)
    //   bits 15-0: register list (bit N = register N)
    //
    // Common forms:
    //   STMFD SP!, {R4-R7, LR}  → push registers (Full Descending = pre-decrement)
    //   LDMFD SP!, {R4-R7, PC}  → pop registers (Full Descending = post-increment)

    fn execute_block_transfer(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let pre = (instruction >> 24) & 1 == 1;
        let up = (instruction >> 23) & 1 == 1;
        let s_bit = (instruction >> 22) & 1 == 1;
        let write_back = (instruction >> 21) & 1 == 1;
        let is_load = (instruction >> 20) & 1 == 1;
        let rn = ((instruction >> 16) & 0xF) as usize;
        let reg_list = instruction & 0xFFFF;
        let has_pc = reg_list & (1 << 15) != 0;

        let base = self.registers[rn];
        let count = reg_list.count_ones();

        // Determine the lowest memory address for the transfer.
        // ARM always stores registers low-to-high in memory regardless of direction.
        //
        //   STMIA/LDMIA (P=0, U=1): lowest = base
        //   STMIB/LDMIB (P=1, U=1): lowest = base + 4
        //   STMDA/LDMDA (P=0, U=0): lowest = base - (count-1)*4
        //   STMDB/LDMDB (P=1, U=0): lowest = base - count*4
        let start_addr = match (pre, up) {
            (false, true)  => base,                                  // IA
            (true,  true)  => base.wrapping_add(4),                  // IB
            (false, false) => base.wrapping_sub((count - 1) * 4),   // DA
            (true,  false) => base.wrapping_sub(count * 4),          // DB
        };

        // S bit with PC NOT in list: access user-mode registers.
        // Temporarily swap to user bank for the transfer, then swap back.
        let current_mode = self.cpsr & 0x1F;
        let use_user_bank = s_bit && !has_pc;
        if use_user_bank && current_mode != 0x10 && current_mode != 0x1F {
            self.switch_mode(current_mode, 0x10);
        }

        // Transfer registers from lowest address upward
        let mut addr = start_addr;
        for i in 0..16u32 {
            if reg_list & (1 << i) != 0 {
                if is_load {
                    self.registers[i as usize] = bus.read_word(addr);
                } else {
                    bus.write_word(addr, self.registers[i as usize]);
                }
                addr = addr.wrapping_add(4);
            }
        }

        // Restore original register bank
        if use_user_bank && current_mode != 0x10 && current_mode != 0x1F {
            self.switch_mode(0x10, current_mode);
        }

        // Write-back the updated base register.
        // For loads: if Rn is in the register list, the loaded value wins (skip write-back).
        let rn_in_list = reg_list & (1 << rn) != 0;
        if write_back && !(is_load && rn_in_list) {
            let new_base = if up {
                base.wrapping_add(count * 4)
            } else {
                base.wrapping_sub(count * 4)
            };
            self.registers[rn] = new_base;
        }

        // If we loaded PC, handle mode switching
        if is_load && has_pc {
            let new_pc = self.registers[R_PC];

            // S bit with PC in list: copy SPSR to CPSR (exception return)
            if s_bit {
                let bank = mode_to_bank(self.cpsr & 0x1F);
                self.write_cpsr(self.spsr[bank], 0xFFFF_FFFF);
            }

            if new_pc & 1 != 0 {
                self.cpsr |= CPSR_T;
                self.registers[R_PC] = new_pc & !1;
            } else {
                self.cpsr &= !CPSR_T;
                self.registers[R_PC] = new_pc & !3;
            }
        }
    }

    // --- Halfword / Signed Byte Transfer (LDRH/STRH/LDSB/LDSH) ---
    // These transfer 16-bit or sign-extended 8-bit values.
    //
    // Encoding (bits 27-26=00, bits 7,4=1,1):
    //   bit 24: P (pre/post)
    //   bit 23: U (add/subtract offset)
    //   bit 22: I (1=immediate offset, 0=register offset)
    //   bit 21: W (write-back)
    //   bit 20: L (load/store)
    //   bits 6-5: SH (00=SWP, 01=unsigned halfword, 10=signed byte, 11=signed halfword)
    //   offset: if I=1, (bits 11-8 << 4) | bits 3-0; if I=0, Rm

    fn execute_halfword_transfer(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        let pre = (instruction >> 24) & 1 == 1;
        let up = (instruction >> 23) & 1 == 1;
        let is_imm = (instruction >> 22) & 1 == 1;
        let write_back = (instruction >> 21) & 1 == 1;
        let is_load = (instruction >> 20) & 1 == 1;
        let rn = ((instruction >> 16) & 0xF) as usize;
        let rd = ((instruction >> 12) & 0xF) as usize;
        let sh = (instruction >> 5) & 0x3;

        let offset = if is_imm {
            ((instruction >> 4) & 0xF0) | (instruction & 0xF)
        } else {
            let rm = (instruction & 0xF) as usize;
            self.registers[rm]
        };

        let base = self.read_reg(rn);
        let addr = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };

        let effective = if pre { addr } else { base };

        if is_load {
            self.registers[rd] = match sh {
                1 => {
                    // LDRH: unaligned address → force-align and rotate right by 8
                    if effective & 1 != 0 {
                        let aligned = effective & !1;
                        let val = bus.read_halfword(aligned) as u32;
                        val.rotate_right(8)
                    } else {
                        bus.read_halfword(effective) as u32
                    }
                }
                2 => bus.read_byte(effective) as i8 as i32 as u32,          // LDRSB
                3 => {
                    // LDRSH: unaligned → loads signed byte instead
                    if effective & 1 != 0 {
                        bus.read_byte(effective) as i8 as i32 as u32
                    } else {
                        bus.read_halfword(effective) as i16 as i32 as u32
                    }
                }
                _ => 0,
            };
        } else {
            // STRH: force-align address (ignore bit 0)
            bus.write_halfword(effective & !1, self.registers[rd] as u16);
        }

        if (write_back || !pre) && !(is_load && rd == rn) {
            self.registers[rn] = addr;
        }
    }

    // --- IRQ (Interrupt Request) entry ---
    // When an interrupt fires, the CPU:
    //   1. Saves CPSR → SPSR_IRQ
    //   2. Switches to IRQ mode (0x12), disables IRQs (I-bit), enters ARM
    //   3. Sets LR_IRQ = return address (PC+4 for ARM, PC+2 for THUMB, adjusted)
    //   4. Jumps to the IRQ vector at 0x00000018 (BIOS)
    fn enter_irq(&mut self) {
        let old_cpsr = self.cpsr;
        let _was_thumb = self.in_thumb_mode();

        // ARM7TDMI IRQ: LR_irq = address of next instruction to execute + 4
        // The handler returns with SUBS PC, LR, #4, giving the correct return address.
        // At this point, self.registers[R_PC] is the address about to be fetched.
        let return_addr = self.registers[R_PC].wrapping_add(4);

        // Switch to IRQ mode (0x12)
        let new_mode = 0x12;
        self.switch_mode(old_cpsr & 0x1F, new_mode);

        // Save CPSR to SPSR_IRQ
        let bank = mode_to_bank(new_mode);
        self.spsr[bank] = old_cpsr;

        // Set LR to return address
        self.registers[R_LR] = return_addr;

        // Update CPSR: IRQ mode, IRQs disabled (I-bit), ARM state (clear T-bit)
        self.cpsr = (old_cpsr & !0xFF) | new_mode | 0x80; // mode=IRQ, I=1, clear T
        self.cpsr &= !CPSR_T;

        // Jump to IRQ vector
        self.registers[R_PC] = 0x00000018;
    }

    // --- SWI (Software Interrupt) — BIOS HLE ---
    // Real GBA: SWI jumps to BIOS code at 0x00000008 which handles the request.
    // We emulate the BIOS functions directly (High Level Emulation / HLE).
    // The function number is in the top 8 bits of the SWI comment field (bits 23-16)
    // in ARM mode, or bits 7-0 in THUMB mode.

    fn execute_swi(&mut self, _instruction: u32, bus: &mut crate::bus::Bus) {
        // In ARM mode, the function number is in bits 23-16
        let comment = (_instruction >> 16) & 0xFF;
        self.handle_bios_call(comment, bus);
    }

    pub fn execute_swi_thumb(&mut self, instruction: u32, bus: &mut crate::bus::Bus) {
        // In THUMB mode, function number is bits 7-0
        let comment = instruction & 0xFF;
        self.handle_bios_call(comment, bus);
    }

    fn handle_bios_call(&mut self, function: u32, bus: &mut crate::bus::Bus) {
        match function {
            0x00 => { /* SoftReset — not implemented */ }
            0x02 => {
                // Halt — CPU sleeps until next interrupt.
                // Advance cycles until an interrupt becomes pending.
                for _ in 0..280_896 {
                    bus.tick(1);
                    if bus.irq_pending() { break; }
                }
            }
            0x04 | 0x05 => {
                // IntrWait (0x04) / VBlankIntrWait (0x05)
                // Wait for interrupt. For VBlankIntrWait, wait specifically for VBlank.
                let start_cycles = bus.cycles;
                loop {
                    bus.tick(1);
                    if bus.irq_pending() { break; }
                    if bus.cycles.wrapping_sub(start_cycles) > 280_896 { break; }
                }
            }
            0x06 => {
                // Div: R0 = R0 / R1, R1 = R0 % R1, R3 = abs(R0/R1)
                let num = self.registers[0] as i32;
                let den = self.registers[1] as i32;
                if den != 0 {
                    self.registers[0] = (num / den) as u32;
                    self.registers[1] = (num % den) as u32;
                    self.registers[3] = (num / den).unsigned_abs();
                }
            }
            0x08 => {
                // Sqrt: R0 = sqrt(R0)
                let val = self.registers[0] as f64;
                self.registers[0] = val.sqrt() as u32;
            }
            0x0B | 0x0C => {
                // CpuSet / CpuFastSet — memory copy/fill
                // R0 = source, R1 = dest, R2 = length/mode
                let src = self.registers[0];
                let dst = self.registers[1];
                let control = self.registers[2];
                let count = control & 0x000F_FFFF;
                let is_fill = (control >> 24) & 1 == 1;
                let is_32bit = function == 0x0C || (control >> 26) & 1 == 1;

                if is_32bit {
                    let fill_val = if is_fill { bus.read_word(src) } else { 0 };
                    for i in 0..count {
                        let val = if is_fill { fill_val } else { bus.read_word(src.wrapping_add(i * 4)) };
                        bus.write_word(dst.wrapping_add(i * 4), val);
                    }
                } else {
                    let fill_val = if is_fill { bus.read_halfword(src) } else { 0 };
                    for i in 0..count {
                        let val = if is_fill { fill_val } else { bus.read_halfword(src.wrapping_add(i * 2)) };
                        bus.write_halfword(dst.wrapping_add(i * 2), val);
                    }
                }
            }
            0x11 | 0x12 => {
                // LZ77UnCompWram (0x11) / LZ77UnCompVram (0x12)
                // R0 = source, R1 = dest
                // Source format: 4-byte header (type/size), then LZ77 compressed stream
                let src = self.registers[0];
                let dst = self.registers[1];
                let header = bus.read_word(src);
                let decomp_size = (header >> 8) & 0x00FF_FFFF;

                let mut src_pos = src + 4;
                let mut dst_pos = dst;
                let mut remaining = decomp_size;

                while remaining > 0 {
                    let flags = bus.read_byte(src_pos);
                    src_pos += 1;

                    for bit in (0..8).rev() {
                        if remaining == 0 { break; }

                        if flags & (1 << bit) == 0 {
                            // Literal byte
                            let b = bus.read_byte(src_pos);
                            src_pos += 1;
                            bus.write_byte(dst_pos, b);
                            dst_pos += 1;
                            remaining -= 1;
                        } else {
                            // Back-reference: 2 bytes -> (length, offset)
                            let b1 = bus.read_byte(src_pos) as u32;
                            let b2 = bus.read_byte(src_pos + 1) as u32;
                            src_pos += 2;

                            let length = ((b1 >> 4) & 0xF) + 3; // 3..18
                            let offset = ((b1 & 0xF) << 8) | b2; // 1..4096
                            let offset = offset + 1;

                            for _ in 0..length {
                                if remaining == 0 { break; }
                                let b = bus.read_byte(dst_pos - offset);
                                bus.write_byte(dst_pos, b);
                                dst_pos += 1;
                                remaining -= 1;
                            }
                        }
                    }
                }
            }
            _ => {
                eprintln!("[WARN] Unknown BIOS call: SWI 0x{:02X}", function);
            }
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
        while cpu.step(&mut bus) != 0 {}
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
    fn mov_lsr_zero_means_lsr_32() {
        // MOV R1, R0, LSR #0 is encoded as LSR #32 → result = 0
        // Encoding: 0xE1A01020 = MOV R1, R0, LSR #0
        let (cpu, _) = run_program(&[
            0xE3A0_0080, // MOV R0, #128
            0xE1A0_1020, // MOV R1, R0, LSR #0 (= LSR #32)
        ]);
        assert_eq!(cpu.registers[1], 0); // 128 >> 32 = 0
    }

    #[test]
    fn mov_asr_zero_means_asr_32() {
        // MOV R1, R0, ASR #0 is encoded as ASR #32 → all sign bits
        // Encoding: 0xE1A01040 = MOV R1, R0, ASR #0
        let (cpu, _) = run_program(&[
            0xE3E0_0000, // MVN R0, #0 → R0 = 0xFFFFFFFF
            0xE1A0_0C00, // MOV R0, R0, LSL #24 → R0 = 0xFF000000
            0xE1A0_1040, // MOV R1, R0, ASR #0 (= ASR #32) → R1 = 0xFFFFFFFF
        ]);
        assert_eq!(cpu.registers[1], 0xFFFF_FFFF);
    }

    #[test]
    fn mov_rrx() {
        // MOV R1, R0, ROR #0 is encoded as RRX (rotate right through carry by 1)
        // Encoding: 0xE1A01060 = MOV R1, R0, RRX
        // R0 = 0x80000001, C = 1 → RRX → carry_out=1, result = (1<<31) | (R0>>1) = 0xC0000000
        // But we need to set carry first. Use MOVS to set carry.
        // ADDS R2, R0, R0 with R0=0x80000000 → carry=1 (overflow)
        let (cpu, _) = run_program(&[
            0xE3A0_0102, // MOV R0, #0x80000000 (imm=0x80, rotate=2 → 0x80000000)
            0xE090_2000, // ADDS R2, R0, R0  → sets carry=1
            0xE3A0_0001, // MOV R0, #1
            0xE1A0_1060, // MOV R1, R0, RRX → result = (C<<31) | (1>>1) = 0x80000000
        ]);
        assert_eq!(cpu.registers[1], 0x8000_0000);
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
