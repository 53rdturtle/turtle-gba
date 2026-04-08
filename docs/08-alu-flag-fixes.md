# Milestone 8: ALU Flag Fixes — Getting the Details Right

## The Problem

With PPU rendering working, armwrestler's test results were finally visible on screen. The "ALU TESTS PART 1" page showed several failures:

```
BIC              BAD Z Rd
CMP              BAD CNZ Rd
MOV              BAD Rd
MVN              BAD CNRd
SBC              BAD C
```

Each error code tells us exactly what's wrong: "BAD Z" means the Zero flag is incorrect, "BAD C" means the Carry flag, "BAD Rd" means the destination register has the wrong value. These are all *flag calculation* bugs — the ALU (Arithmetic Logic Unit) computes the right answer but sets the wrong status flags.

## Bug 7: Barrel Shift Special Encodings

**Affected tests**: BIC, CMP, MVN (all showed "BAD Z Rd" or "BAD CNRd")

### What's a Barrel Shifter?

The ARM7TDMI has a **barrel shifter** — a hardware circuit that can shift or rotate a value by any amount in a single cycle. It sits between the register file and the ALU, so every data processing instruction can optionally shift its second operand before the ALU sees it.

There are four shift types:
- **LSL** (Logical Shift Left): Shifts bits left, fills with 0s. `5 LSL 1 = 10`
- **LSR** (Logical Shift Right): Shifts bits right, fills with 0s. `8 LSR 2 = 2`
- **ASR** (Arithmetic Shift Right): Shifts right, preserving the sign bit. `-8 ASR 1 = -4`
- **ROR** (Rotate Right): Bits that fall off the right wrap to the left.

### The Encoding Trap

ARM instructions encode the shift amount in two ways:

1. **Immediate shift** (bit 4 = 0): A 5-bit field (bits 11-7) specifies the amount (0-31)
2. **Register shift** (bit 4 = 1): A register's bottom byte specifies the amount (0-255)

The trap: when the 5-bit immediate field is 0, the hardware does NOT mean "shift by 0." Instead, it uses special encodings because "shift by 0" would be redundant (that's just the unshifted register). The ARM designers repurposed these bit patterns:

```
Encoding      Immediate (5-bit = 0)     Register (Rs = 0)
─────────     ─────────────────────     ─────────────────
LSL #0        → LSL #0 (identity)      → LSL #0 (identity)
LSR #0        → LSR #32 (result = 0)   → no shift (identity)
ASR #0        → ASR #32 (all sign)     → no shift (identity)
ROR #0        → RRX (rotate through    → no shift (identity)
                carry, shift by 1)
```

**RRX** (Rotate Right eXtended) deserves special mention. It's a 33-bit rotation: the carry flag becomes bit 32, and all 33 bits rotate right by one position. The old bit 0 becomes the new carry, and the old carry becomes bit 31 of the result. This is useful for multi-word arithmetic — it propagates the carry between words.

```
Before:  C=1, value = 0b01010110
         ↓
RRX:     C=0, value = 0b10101011
         (old C shifted into bit 31, old bit 0 shifted into C)
```

### What Was Wrong

Our barrel shifter treated amount=0 the same way for both immediate and register shifts — it always returned the value unchanged. This meant:

- `MOV R0, R1, LSR #0` should compute `R1 LSR 32` (result = 0), but we returned R1 unchanged
- `BIC R0, R1, R2, ASR #0` should compute `R1 AND NOT(R2 ASR 32)`, but we used the wrong operand
- The wrong operand values led to wrong results AND wrong flags

### The Fix

We split the barrel shifter into `barrel_shift_ext` with an `is_immediate_shift` parameter:

```rust
fn barrel_shift_ext(&self, value: u32, shift_type: u32, amount: u32,
                    is_immediate_shift: bool) -> (u32, bool) {
    if amount == 0 {
        if !is_immediate_shift || shift_type == 0 {
            return (value, carry_in); // Identity for reg-shift or LSL#0
        }
        return match shift_type {
            1 => (0, (value >> 31) & 1 != 0),           // LSR #32
            2 => {                                        // ASR #32
                let sign = (value as i32) >> 31;
                (sign as u32, (value >> 31) & 1 != 0)
            },
            3 => {                                        // RRX
                let carry = value & 1 != 0;
                let result = (value >> 1) | ((carry_in as u32) << 31);
                (result, carry)
            },
            _ => unreachable!(),
        };
    }
    // ... normal shift logic for amount > 0
}
```

## Bug 8: SBC Carry Flag

**Affected test**: SBC showed "BAD C"

### How Subtraction Really Works in Hardware

The ARM7TDMI doesn't have a subtraction circuit. Instead, it reuses the **adder** for everything. To compute `A - B`, the hardware actually computes:

```
A + NOT(B) + 1
```

This works because of two's complement: `NOT(B) + 1 = -B`, so `A + NOT(B) + 1 = A - B`.

**SBC** (Subtract with Carry) is the multi-word version. When subtracting large numbers that span multiple registers, you need to propagate the borrow from the low word to the high word. SBC computes:

```
A + NOT(B) + C
```

Where C is the carry flag from the previous operation. (Note: C=1 means "no borrow", C=0 means "borrow occurred" — the opposite of what you might expect, because carry and borrow are inverses.)

### The Carry Flag Rule

The carry flag after SBC is set when the result **doesn't borrow** — that is, when the unsigned result fits in 32 bits. The hardware computes this with 64-bit arithmetic:

```
wide = (A as u64) + (NOT(B) as u64) + (C as u64)
carry_out = wide > 0xFFFF_FFFF
```

### What Was Wrong

Our code computed the carry flag as simply `A >= B`, ignoring the incoming carry flag entirely. This is wrong for SBC because:

- If C=1 (no borrow): `5 - 3` with C=1 → `5 + NOT(3) + 1 = 5 + 0xFFFFFFFC + 1 = overflow → carry=1` ✓
- If C=0 (borrow): `5 - 3` with C=0 → `5 + NOT(3) + 0 = 5 + 0xFFFFFFFC = overflow → carry=1` ✓
- But: `3 - 3` with C=0 → `3 + NOT(3) + 0 = 3 + 0xFFFFFFFC = 0xFFFFFFFF → carry=0` (borrow!)
  - Our old code: `3 >= 3 → carry=1` ❌

### The Fix

Use proper 64-bit wide arithmetic matching the hardware:

```rust
// SBC: Rd = op1 + NOT(op2) + C
let c = if self.flag_c() { 1u64 } else { 0u64 };
let wide = op1 as u64 + (!op2) as u64 + c;
let result = wide as u32;
self.set_flag(CPSR_C, wide > 0xFFFF_FFFF);
```

The same pattern applies to RSC (Reverse Subtract with Carry) with operands swapped, and to ADC (Add with Carry) which is simpler: `wide = A + B + C`.

## Bug 9: MOV and the PC+12 Quirk

**Affected test**: MOV showed "BAD Rd"

### The ARM7TDMI Pipeline

The ARM7TDMI has a 3-stage pipeline:

```
Stage 1: FETCH    — read the next instruction from memory
Stage 2: DECODE   — figure out what the instruction does
Stage 3: EXECUTE  — do the actual work
```

These stages overlap. While instruction N is executing, instruction N+1 is being decoded, and instruction N+2 is being fetched. This means the Program Counter (PC / R15) always points to the instruction being *fetched*, which is 2 instructions ahead of the one being *executed*:

```
In ARM mode (4 bytes per instruction):
  PC during execution = instruction_address + 8

In THUMB mode (2 bytes per instruction):
  PC during execution = instruction_address + 4
```

### The Extra Cycle

Here's where it gets subtle. When an instruction uses **operand2 with a register-specified shift** (bit 4 = 1), the CPU needs an extra internal cycle to read the shift register. During this extra cycle, the prefetcher advances PC by one more instruction word:

```
Normal:           PC reads as instruction_addr + 8
Register shift:   PC reads as instruction_addr + 12
```

This only happens when:
1. The instruction is a data processing operation (AND, ORR, MOV, etc.)
2. Operand2 uses a register (not immediate)
3. The shift amount comes from a register (bit 4 = 1), not an immediate field

### What Armwrestler Tests

Armwrestler's MOV test does exactly this:

```asm
MOV  R3, R15          @ R3 = PC (should be instruction_addr + 8)
MOVS R4, R15, LSL R3  @ R4 = PC << R3 (PC should be instruction_addr + 12!)
                       @ Since R3 was set to make LSL amount = 0,
                       @ R4 should just equal instruction_addr + 12
```

The second instruction uses a register-specified shift (`LSL R3`), so when it reads R15 (PC) as the Rm operand, it gets the +12 value. If the emulator returns +8, R4 will be wrong → "BAD Rd".

### The Fix

In `decode_operand2`, when `shift_by_reg` is true and Rm is PC, add an extra 4:

```rust
let rm_val = if shift_by_reg && rm == R_PC {
    self.read_reg(rm).wrapping_add(4) // PC+12 for register-specified shift
} else {
    self.read_reg(rm)
};
```

## Lessons Learned

1. **Read the instruction encoding carefully** — a single bit (bit 4: immediate vs register shift) completely changes how the barrel shifter interprets "amount = 0". This is a theme throughout ARM: dense bit-packing means every bit matters.

2. **Model the hardware, not the math** — for SBC's carry flag, the "obvious" formula (`A >= B`) is wrong. The correct formula comes from understanding that the hardware uses an adder with `NOT(B)`, not a subtractor. When in doubt, think about what the actual circuit does.

3. **Pipeline quirks are real** — the PC+12 behavior for register shifts seems like a bug, but it's documented behavior. It exists because the extra cycle to read the shift register lets the prefetcher run one more time. ARM could have hidden this, but the ARM7TDMI is a simple in-order core that exposes pipeline timing to the programmer. Later ARM cores (ARMv5+) eliminated some of these quirks.

4. **Test ROMs are invaluable** — armwrestler's error codes (BAD C, BAD Z, BAD Rd) pointed us directly to the broken flag or register, making debugging much faster than guessing from visual output.
