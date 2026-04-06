# Milestone 6: Banked Registers, LDM/STM Fix, and BIOS Boot

## The Three-Bug Chain

Running the open-source BIOS (Cult-of-GBA/BIOS — Basic Input/Output System, the GBA's built-in startup code) exposed three interconnected bugs that taught us fundamental things about how the ARM7TDMI CPU actually works.

## Bug 1: MSR Doesn't Know CPSR from SPSR

**Symptom**: BIOS boot loops infinitely, never getting past the first 10 instructions.

**What happened**: The `MSR` (Move to Status Register) instruction writes to a status register. Bit 22 of the instruction determines which one:
- Bit 22 = 0 → write to **CPSR (Current Program Status Register)** — the active flags and mode
- Bit 22 = 1 → write to **SPSR (Saved Program Status Register)** — a backup copy saved when entering a special mode

Our code ignored bit 22 and always wrote to CPSR. When the BIOS did `MSR SPSR, R14` (saving a value for later), we overwrote CPSR with 0, resetting the processor mode bits and causing an infinite restart.

**Fix**: Check bit 22. If it's 1, write to the current mode's SPSR bank instead of CPSR.

**Lesson**: On real hardware, the CPSR and SPSR are physically different registers. The instruction encoding uses a single bit to select between them. When you miss one bit in an instruction decoder, the whole system can break in subtle ways.

## Bug 2: The ARM7TDMI Has 37 Registers, Not 16

**Symptom**: After fixing Bug 1, the BIOS clears I/O registers successfully but then jumps back to address 0x00000000 instead of continuing.

**What happened**: The ARM7TDMI has 6 processor modes, and most have their own private copies of R13 (SP — Stack Pointer) and R14 (LR — Link Register). "Banked" means the CPU automatically swaps in a different physical register when it changes mode, like having separate drawers labeled for each job:

```
                R0-R7  R8-R12  R13(SP)  R14(LR)  SPSR
  User/System:  shared shared  SP_usr   LR_usr   (none)
  Supervisor:   shared shared  SP_svc   LR_svc   SPSR_svc
  IRQ:          shared shared  SP_irq   LR_irq   SPSR_irq
  FIQ:          shared R8-R12  SP_fiq   LR_fiq   SPSR_fiq
  Abort:        shared shared  SP_abt   LR_abt   SPSR_abt
  Undefined:    shared shared  SP_und   LR_und   SPSR_und

  (IRQ = Interrupt Request, FIQ = Fast Interrupt Request,
   SPSR = Saved Program Status Register — backup of CPSR)
```

That's 16 visible + 10 banked SP/LR + 5 banked FIQ R8-R12 + 5 SPSRs + 1 CPSR = **37 registers**.

The BIOS does:
1. `BL init_stacks` → sets LR = 0x30 (return address) in System mode
2. `MSR CPSR, #0x13` → switch to Supervisor mode
3. `MOV R14, #0` → clear Supervisor's LR (should NOT touch System's LR)
4. ... set up other modes ...
5. `MSR CPSR, #0x1F` → switch back to System mode
6. `BX LR` → should return to 0x30

Without banked registers, step 3 destroyed the ONLY LR, so step 6 jumped to 0 instead of 0x30.

**Fix**: Added `banked_sp[6]`, `banked_lr[6]`, `banked_fiq_r8_r12[5]`, and `spsr[6]` arrays. When CPSR mode bits change (via MSR or `write_cpsr`), we save the current SP/LR to the old bank and load from the new bank.

**Lesson**: This is WHY modes exist — they give interrupt handlers their own stack and link register so they can't corrupt the main program's state. The hardware does this register swapping in a single clock cycle, which is remarkably efficient.

## Bug 3: LDM/STM (Load/Store Multiple) Was Off by 4 Bytes

**Symptom**: After implementing banked registers, the BIOS still loops — function calls (via PUSH/POP — Push registers onto stack / Pop registers from stack) return to the wrong address.

**What happened**: ARM block transfers (LDM/STM) have 4 addressing modes determined by two bits:

```
  STMIA (Store Multiple Increment After,  P=0, U=1): store at base, base+4, base+8...
  STMIB (Store Multiple Increment Before, P=1, U=1): store at base+4, base+8, base+12...
  STMDA (Store Multiple Decrement After,  P=0, U=0): store at base, base-4, base-8...
  STMDB (Store Multiple Decrement Before, P=1, U=0): store at base-4, base-8, base-12...
```

PUSH uses STMDB (pre-decrement), POP uses LDMIA (Load Multiple Increment After — post-increment).

Our old code computed the start address for descending mode, then applied the pre-index offset *again* in the loop, shifting every register store by 4 bytes. So PUSH stored LR at the wrong spot, and POP loaded garbage.

**Fix**: Rewrote LDM/STM to compute the correct lowest address for each mode, then always store registers ascending from there:

```rust
let start_addr = match (pre, up) {
    (false, true)  => base,                               // IA
    (true,  true)  => base.wrapping_add(4),               // IB
    (false, false) => base.wrapping_sub((count - 1) * 4), // DA
    (true,  false) => base.wrapping_sub(count * 4),        // DB
};
```

**Lesson**: LDM/STM is one of the trickiest ARM instructions to get right. The 4 addressing modes seem similar but the address calculations are subtly different. Getting PUSH/POP wrong breaks ALL function calls.

## BIOS Skip

Even after fixing all three bugs, the BIOS boot takes millions of cycles because it initializes sound hardware using lookup tables and computation loops. Most emulators **skip the BIOS boot** and set up the post-boot state directly:

```
PC = 0x08000000    (ROM — Read-Only Memory, the game cartridge — entry point)
CPSR = 0x1F        (System mode)
SP = 0x03007F00    (System stack)
SP_svc = 0x03007FE0 (Supervisor stack)
SP_irq = 0x03007FA0 (IRQ stack)
```

The BIOS binary is still loaded in memory so SWI (Software Interrupt — how games call built-in BIOS functions) calls can use it.

## Log-Driven Debugging

These bugs were found using **log-driven debugging** — adding targeted trace output at decision points and letting the data show what's wrong:

1. For Bug 1: traced the first 20 steps and saw CPSR reset to 0 at step 8
2. For Bug 2: traced steps 398-403 and saw BX LR jumping to 0 instead of 0x30
3. For Bug 3: added BIOS address logging and saw the code never reaching 0x38

The pattern: **narrow down with broad logging, then zoom in with targeted logging**.

## Bug 4: THUMB BL Return Address Off by 2

**Symptom**: After fixing Bugs 1-3 and skipping the BIOS boot, armwrestler runs 196K instructions then jumps to BIOS address 0 via `BX R3` (Branch and eXchange — jump to address and potentially switch between ARM/THUMB modes) where R3=0.

**What happened**: THUMB (the 16-bit compressed instruction set) `BL` (Branch with Link — function call) is a two-instruction sequence. The second instruction must set LR to point to the instruction *after* the BL pair (the return address). Our code computed:
```rust
LR = (PC - 2) | 1  // ← BUG: points AT the BL second half!
```
But PC was already advanced to instruction_addr + 2, so `PC - 2` is the BL second half itself. When the called function returned via `BX LR`, it re-executed the BL second half, creating an accidental chain of jumps.

**Fix**: `LR = PC | 1` (PC is already the correct return address).

**Lesson**: This is a classic off-by-one in pipeline handling. ARM's THUMB BL needs the return address to be the instruction after the two-instruction pair, but the PC only advances by 2 for each half.

## Bug 5: VBlank Polling (Solved with PPU Stub)

**Symptom**: Armwrestler runs its initialization correctly but then loops forever at 0x080004DC-0x080004E4.

**What happened**: The code reads DISPSTAT (Display Status — 0x04000004) in a tight loop, checking bit 0 (VBlank flag). VBlank (Vertical Blank) is the period when the screen has finished drawing all visible scanlines and the beam is returning to the top — games use this pause to safely update graphics. On real hardware, the PPU (Pixel Processing Unit — the graphics rendering hardware) sets this flag every 280,896 cycles when the screen finishes drawing. Without any PPU emulation, the flag is always 0.

**Fix**: Added a `tick()` method to the Bus that counts cycles and updates:
- **VCOUNT** (Vertical Count — 0x04000006): current scanline number (0-227)
- **DISPSTAT** (0x04000004): VBlank/HBlank (Horizontal Blank — the short pause at the end of each scanline)/VCount match flags

This isn't a full PPU, but it's enough for games to progress past VBlank waits.

## Current State

With all 5 bugs fixed, armwrestler:
1. Starts at ROM entry point (BIOS skip)
2. Initializes display and interrupt mode registers
3. Switches to THUMB mode
4. Clears 256KB of EWRAM (External Work RAM — extra general-purpose memory, ~196K instructions)
5. Switches back to ARM mode
6. **Runs the actual ARM instruction tests!** (millions of instructions)

The next step is real PPU rendering so we can see armwrestler's test results on screen.

## What's Next

- PPU (graphics) — render armwrestler's test results to a window
- More I/O register handling (DMA — Direct Memory Access, hardware that copies memory without CPU involvement; timers)
- SWI exception handling (for BIOS function calls from running games)
