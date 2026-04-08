# Milestone 10: Load/Store Fixes and THUMB Barrel Shift Corrections

After getting the PPU rendering and input handling working, we could finally run armwrestler's full test suite — 6 pages covering ARM ALU, ARM LDR/STR, ARM LDM/STM, THUMB ALU, THUMB LDR/STR, and THUMB LDM/STM. This exposed a wave of bugs, all rooted in subtle hardware behaviors that are easy to get wrong.

## Bug 10: Write-Back vs. Loaded Value (Rd == Rn)

**Affected tests**: ARM LDR/STR — all negative-offset variants showed "BAD Rd"

### The Hardware Problem

ARM load instructions like `LDR R0, [R0, #4]!` do two things:
1. **Load**: read a word from memory at `R0 + 4` and put it in R0
2. **Write-back** (the `!`): update R0 to `R0 + 4`

But what happens when the destination register (Rd) is the same as the base register (Rn)? Both the load and the write-back want to write to the same register. The ARM7TDMI resolves this simply: **the loaded value wins**. Write-back is suppressed when Rd == Rn on a load.

```
LDR R0, [R0, #4]!
  Step 1: addr = R0 + 4
  Step 2: R0 = mem[addr]     ← load happens
  Step 3: R0 = addr          ← write-back would happen... but Rd == Rn, so skip!
  Result: R0 contains the loaded value, not the computed address
```

### Why Our Code Was Wrong

Our code did the load first, then unconditionally wrote back the address:

```rust
// Load
self.registers[rd] = bus.read_word(effective_addr);
// Write-back (OVERWRITES the loaded value when rd == rn!)
if !pre_index || write_back {
    self.registers[rn] = addr;
}
```

The fix adds a guard:

```rust
if (!pre_index || write_back) && !(is_load && rd == rn) {
    self.registers[rn] = addr;
}
```

### Why Only Negative Offsets Failed

This seems odd at first — why would the U bit (add/subtract) matter? It's because armwrestler's negative-offset tests happen to use `Rd == Rn` patterns more than the positive-offset tests. The bug exists for positive offsets too, but armwrestler's specific test patterns didn't trigger it there.

## Bug 11: LDM Write-Back with Rn in Register List

**Affected tests**: ARM LDM/STM — LDMIB, LDMDB, LDMDA showed "BAD Rn"

### Block Transfer Basics

LDM (Load Multiple) and STM (Store Multiple) transfer a set of registers to/from consecutive memory addresses in a single instruction. They're the ARM equivalent of push/pop, but more general — you can load or store any subset of registers.

The instruction encodes a 16-bit register list where each bit corresponds to a register (bit 0 = R0, bit 1 = R1, ..., bit 15 = R15/PC). The P (Pre/Post) and U (Up/Down) bits determine the addressing direction:

```
LDMIA  (Increment After):   base, base+4, base+8, ...
LDMIB  (Increment Before):  base+4, base+8, base+12, ...
LDMDA  (Decrement After):   ..., base-8, base-4, base
LDMDB  (Decrement Before):  ..., base-12, base-8, base-4
```

With write-back (`!`), the base register is updated to point past the transferred block.

### The Same Rd == Rn Problem, Scaled Up

Just like single LDR, if the base register (Rn) is included in the register list of an LDM with write-back, both the load and the write-back want to set Rn. The ARM7TDMI rule is the same: **the loaded value wins**.

```
LDMIA R0!, {R0, R1, R2}
  mem[R0+0] → R0  (loaded value)
  mem[R0+4] → R1
  mem[R0+8] → R2
  R0 = R0 + 12    ← skipped because R0 was in the list!
```

Our fix:

```rust
let rn_in_list = reg_list & (1 << rn) != 0;
if write_back && !(is_load && rn_in_list) {
    self.registers[rn] = new_base;
}
```

## Bug 12: THUMB LDR Unaligned Access

**Affected tests**: THUMB LDR/STR — LDR showed "BAD Rd"

### How ARM7TDMI Handles Misaligned Reads

The ARM7TDMI bus is 32 bits wide. When you read a word, the bottom 2 bits of the address are ignored — the bus always reads from a 4-byte-aligned address. But the CPU doesn't just return the aligned word; it **rotates** the result so the byte at your requested address ends up in the least significant position:

```
Memory at 0x100: [AA BB CC DD]  (word at aligned address 0x100)

LDR from 0x100: 0xDDCCBBAA     (aligned, no rotation)
LDR from 0x101: 0xAADDCCBB     (rotated right by 8 bits)
LDR from 0x102: 0xBBAADDCC     (rotated right by 16 bits)
LDR from 0x103: 0xCCBBAADD     (rotated right by 24 bits)
```

This rotation behavior is the same in both ARM and THUMB mode. We had already implemented it for ARM LDR in an earlier milestone, but THUMB LDR was doing a naive `bus.read_word(addr)` without the force-align and rotate logic.

### The Fix

Applied the same unaligned rotation to both THUMB Format 7 (register offset) and Format 9 (immediate offset):

```rust
// Before (broken)
bus.read_word(addr)

// After (correct)
let misalign = addr & 3;
let aligned = addr & !3;
let val = bus.read_word(aligned);
if misalign != 0 { val.rotate_right(misalign * 8) } else { val }
```

### Why This Matters

Games sometimes compute addresses that end up slightly misaligned due to pointer arithmetic. A correct emulator must handle this exactly as the hardware does, or data will be garbled — bytes from adjacent words will appear in the wrong positions, causing corrupted graphics, wrong values, and hard-to-diagnose bugs.

LDRH (halfword load) and LDRSH (signed halfword load) have their own quirky misalignment rules, which we implemented earlier: LDRH from an odd address force-aligns and rotates by 8, while LDRSH from an odd address loads a signed byte instead.

## Bug 13: THUMB Barrel Shift — Immediate vs. Register Encoding

**Affected tests**: THUMB ALU — ROR showed "BAD C Rd"

### The Two Faces of "Shift by Zero"

We learned in Milestone 8 that the ARM barrel shifter interprets "shift amount = 0" differently depending on whether the amount comes from a 5-bit immediate field or a register:

| Shift | Immediate (amount = 0) | Register (amount = 0) |
|-------|------------------------|----------------------|
| LSL   | Identity (no shift)    | Identity (no shift)  |
| LSR   | LSR #32 (result = 0)   | No shift (identity)  |
| ASR   | ASR #32 (sign-fill)    | No shift (identity)  |
| ROR   | RRX (rotate through C) | No shift (identity)  |

The critical difference: **immediate shift amount 0 triggers special encodings, register shift amount 0 means "don't shift."**

### Where THUMB Shifts Come From

THUMB ALU operations (Format 4) that involve shifts — LSL, LSR, ASR, ROR — all use **register-specified** shift amounts. The bottom byte of Rs specifies how many bits to shift. When Rs contains 0, the result should be the unchanged input value.

But our THUMB code was calling `barrel_shift()`, a convenience wrapper that passes `is_immediate_shift: true`:

```rust
// Wrong: treats shift amount as 5-bit immediate encoding
0x7 => { let (r, c) = self.barrel_shift(a, 3, b & 0xFF); ... }

// Right: treats shift amount as register value
0x7 => { let (r, c) = self.barrel_shift_ext(a, 3, b & 0xFF, false); ... }
```

When armwrestler tested `ROR R1, R0` with R0 = 0, our code performed RRX (Rotate Right eXtended — a 33-bit rotation through the carry flag) instead of returning the value unchanged. This corrupted both the result (Rd) and the carry flag (C).

### The Deeper Lesson

This is the same barrel shift bug from Milestone 8, but in a different context. The `barrel_shift()` wrapper was convenient but hid the implicit assumption that the shift amount was from an immediate field. THUMB Format 4 shifts always use register amounts, so they need `barrel_shift_ext(..., false)`.

The fix affected all four THUMB ALU shift operations (LSL, LSR, ASR, ROR), even though only ROR triggered a test failure. The others would have failed too if armwrestler tested them with a zero shift amount — we fixed them preemptively.

## Bug 14: THUMB MUL Carry Flag

**Not tested by armwrestler** (fixed preemptively)

The ARM7TDMI's multiply instruction "destroys" the carry flag — after MUL, the C flag contains a meaningless value related to the internal Booth multiplier algorithm. Different hardware revisions produce different values.

Most emulators handle this by setting C to 0 after MUL. We applied this to the THUMB MUL operation:

```rust
0xD => {
    self.set_flag(CPSR_C, false);  // C destroyed
    a.wrapping_mul(b)
}
```

## Automated Testing

To iterate faster on these fixes, we built an `--auto-test` mode that navigates armwrestler's menu automatically:

```
cargo run -- --auto-test roms/armwrestler.gba
```

This enters each of armwrestler's 6 test categories (ARM ALU, ARM LDR/STR, ARM LDM/STM, THUMB ALU, THUMB LDR/STR, THUMB LDM/STM), captures a BMP screenshot of each, and exits. Combined with a Python script to convert BMPs to PNGs for viewing, this gives us a one-command test cycle.

Key design decisions:
- **Button presses are simulated** via the `press_button()` helper that sets KEYINPUT for N frames then releases
- **Timing matters**: we hold each button for 10 frames and wait 60 frames between actions, since armwrestler only checks input during VBlank
- **Select returns to menu**: armwrestler uses Select (not Start) to go back from a test page to the menu

## Bug 15: LDM/STM S-Bit — Privileged Register Access

**Affected tests**: ARM LDM/STM — LDMIBS!, LDMIAS!, LDMDBS!, LDMDBA! showed "BAD Rd"

### ARM7TDMI Privilege Modes

The ARM7TDMI has 6 operating modes, each with its own banked copies of certain registers:

```
Mode        Bits   Banked Registers           Purpose
────        ────   ─────────────────          ───────
User        0x10   (base set)                 Normal program execution
FIQ         0x11   R8-R12, R13, R14, SPSR     Fast interrupt
IRQ         0x12   R13, R14, SPSR             Normal interrupt
Supervisor  0x13   R13, R14, SPSR             SWI / OS calls
Abort       0x17   R13, R14, SPSR             Memory faults
Undefined   0x1B   R13, R14, SPSR             Undefined instructions
System      0x1F   (shares User's banks)      Privileged User mode
```

When the CPU switches from User to IRQ mode (e.g., on an interrupt), the hardware automatically swaps R13 and R14 to the IRQ bank. The user's R13/R14 are preserved in the User bank and restored when returning.

### What the S-Bit Does

The S bit (bit 22) in LDM/STM instructions has two distinct behaviors:

**Case 1: LDM with S=1 and PC in the register list**
This is an "exception return" — load registers AND copy the current mode's SPSR (Saved Program Status Register) back to CPSR. This restores both the register values and the CPU mode in a single instruction. Used at the end of interrupt handlers:

```asm
LDMFD SP!, {R0-R3, PC}^    @ ^ = S bit
@ Loads R0-R3 and PC from stack
@ Copies SPSR_irq → CPSR (switches back to User mode)
```

**Case 2: LDM/STM with S=1 and PC NOT in the register list**
Access **User-mode registers** instead of the current mode's banked registers. This lets a privileged mode (like Supervisor) save or restore a user task's registers:

```asm
@ In Supervisor mode:
STMFD SP!, {R13-R14}^      @ Store User's SP and LR, not Supervisor's!
LDMFD SP!, {R13-R14}^      @ Load into User's SP and LR
```

Without the S bit, this would access Supervisor's own R13/R14, which is wrong when you're trying to save the user's context.

### The Implementation

For Case 2, we temporarily swap to user-mode register banks before the transfer, then swap back:

```rust
let use_user_bank = s_bit && !has_pc;
if use_user_bank && current_mode != 0x10 && current_mode != 0x1F {
    self.switch_mode(current_mode, 0x10); // Swap to user banks
}

// ... do the transfer (reads/writes user-mode R13/R14) ...

if use_user_bank && current_mode != 0x10 && current_mode != 0x1F {
    self.switch_mode(0x10, current_mode); // Swap back
}
```

For Case 1, after loading all registers (including PC), we copy SPSR to CPSR:

```rust
if is_load && has_pc && s_bit {
    let bank = mode_to_bank(self.cpsr & 0x1F);
    self.write_cpsr(self.spsr[bank], 0xFFFF_FFFF);
}
```

The `write_cpsr` call handles the bank switch automatically when the mode bits change.

## Current Test Results

```
ARM ALU:        ALL OK (14/14)
ARM LDR/STR:    ALL OK
ARM LDM/STM:    ALL OK (12/12) ← S-bit variants now pass!
THUMB ALU:      ALL OK (11/11)
THUMB LDR/STR:  ALL OK (7/7)
THUMB LDM/STM:  ALL OK (2/2)
```

**Perfect score on armwrestler.** Every test across all 6 pages passes.

## Lessons Learned

1. **Write-back conflicts are a pattern**: whenever an instruction both loads a register AND modifies the same register through a side effect (write-back, auto-increment), you need to decide who wins. On the ARM7TDMI, the loaded value always takes priority.

2. **Don't assume symmetry**: our positive-offset LDR tests passed while negative-offset failed, not because of the subtraction itself, but because the test cases happened to use Rd == Rn patterns more in the negative variants. Bug hunting requires testing all combinations, not just assuming "if + works, - works too."

3. **Wrapper functions can hide assumptions**: `barrel_shift()` looked safe but encoded an assumption (`is_immediate_shift: true`) that was wrong for THUMB register shifts. When wrapping a function that has modal behavior, make sure the wrapper's default matches ALL call sites.

4. **Automated testing pays off immediately**: building the `--auto-test` mode cost ~30 minutes but saved hours of manual navigation. When you're iterating on CPU accuracy, the fix-build-test cycle needs to be fast.
