# Milestone 12: Interrupts, BIOS Fixes, and Lessons from a Real Game

We tried running Wario Land 4 and hit a cascade of bugs. Each fix revealed another missing piece. After fixing all the issues, **the game boots and renders its intro scene** — a blue corridor with perspective tiles. This milestone covers the bugs we found and how the GBA interrupt system works.

## Bug 1: THUMB SWI Misidentified as Conditional Branch

**The symptom**: The game got stuck in an infinite byte-copy loop with R3=0xFFFFFFFF (4 billion iterations). Register values were garbage — I/O addresses where data should be.

**The cause**: THUMB `SWI` instructions (opcode `0xDFxx`) share the same top-5 bits (`0b11011`) as conditional branches (`0xD0xx`–`0xDExx`). Our decoder checked for conditional branches first, so SWI was never reached:

```
0xDF0B = SWI 0x0B (CpuSet)
         1101 1111 0000 1011
         ^^^^^ 
         top5 = 11011  ← matched conditional branch before SWI!
```

**The fix**: Check for SWI (`top8 == 0xDF`) *before* checking for conditional branches. The condition field `0xF` in a "conditional branch" actually means SWI on the ARM7TDMI — it's not a valid condition code for branches.

**The lesson**: When multiple instruction formats share bit patterns, decode order matters. Always check the more specific pattern first. This is a recurring theme in CPU emulation — the instruction encoding space is packed tight, and overlaps are intentional.

## Bug 2: THUMB MOV PC, LR Leaves PC Misaligned

**The symptom**: After fixing SWI dispatch, the game made BIOS calls correctly but later jumped to a data section and executed garbage.

**The cause**: `MOV PC, LR` in THUMB Format 5 (Hi register operations) was copying the Link Register (LR) directly to the Program Counter (PC) without masking bit 0. On the GBA, LR values for THUMB return addresses have bit 0 set (indicating THUMB state). Our code stored this odd value directly into PC, causing every subsequent instruction fetch to be misaligned by one byte.

```rust
// Before (broken):
self.registers[rd] = rs_val;  // PC gets odd value like 0x0800224F

// After (fixed):
if rd == R_PC {
    self.registers[R_PC] = rs_val & !1;  // Force halfword alignment
} else {
    self.registers[rd] = rs_val;
}
```

**The lesson**: Any instruction that writes to PC needs special handling. On the ARM7TDMI, PC writes in THUMB mode must be halfword-aligned (clear bit 0). Only `BX` uses bit 0 for ARM/THUMB switching — other PC-writing instructions just need alignment.

## Bug 3: No Interrupt System

**The symptom**: After both fixes above, the game called `SWI 0x02` (Halt) in an infinite loop, never making progress.

**The cause**: `Halt` is supposed to pause the CPU until a hardware interrupt (IRQ) fires. Without an interrupt system, Halt returned immediately, the game checked if the interrupt it was waiting for had occurred (it hadn't), and called Halt again.

### How GBA Interrupts Work

The GBA's interrupt system involves three I/O registers and the CPU's mode system:

```
Register    Address        Purpose
────────    ───────        ───────
IE          0x04000200     Interrupt Enable — which interrupt sources are active
IF          0x04000202     Interrupt Flags — which interrupts have fired (write-1-to-clear)
IME         0x04000208     Interrupt Master Enable — global on/off switch
```

**Interrupt sources** (bits in IE/IF):
- Bit 0: VBlank — start of vertical blanking period
- Bit 1: HBlank — start of horizontal blanking period  
- Bit 2: VCount — scanline counter matches target value
- Bit 3: Timer 0 overflow
- Bit 4-6: Timer 1-3 overflow
- Bit 8-11: Serial, DMA, Keypad, Game Pak

**The flow when an interrupt fires**:
1. Hardware sets the corresponding bit in IF
2. CPU checks: is `IME` enabled? Is `(IE & IF) != 0`? Is the CPSR I-bit clear (IRQs enabled)?
3. If all yes: save CPSR → SPSR_IRQ, switch to IRQ mode, disable IRQs, jump to 0x00000018
4. The BIOS handler at 0x00000018 reads the game's registered handler from 0x03007FFC
5. The game's handler processes the interrupt, acknowledges it by writing to IF
6. Returns via `SUBS PC, LR, #4` which restores CPSR from SPSR_IRQ

**IF is write-1-to-clear**: Writing a 1 to a bit *clears* it (acknowledges the interrupt). This prevents the common "write to clear" race condition where reading and clearing aren't atomic. You write exactly the bits you've handled.

**DISPSTAT gates interrupt signals**: The PPU only raises VBlank/HBlank/VCount interrupts if the corresponding enable bits (bits 3-5) in DISPSTAT (0x04000004) are set. This is separate from IE — both must be enabled for the interrupt to fire.

### What We Implemented

We added:
1. **Interrupt flag setting** in `tick()` — sets IF bits on VBlank/HBlank/VCount rising edges
2. **IRQ pending check** — `irq_pending()` returns true when IME && (IE & IF) != 0
3. **IRQ entry** — `enter_irq()` saves state, switches to IRQ mode, jumps to 0x00000018
4. **Halt HLE** — advances cycles until `irq_pending()` returns true
5. **IF write-1-to-clear** — writing to IF clears the corresponding bits

## Bug 4: IRQ Return Address Off by 2 in THUMB Mode

**The symptom**: After implementing interrupts, the IRQ handler ran correctly — it read IE&IF, dispatched to the VBlank handler, wrote the game's custom flag at 0x03000C42 — but after returning, the game landed 2 bytes too early, executing data as code before falling into the real function.

**The cause**: When entering IRQ mode, we set `LR_irq = PC + 2` for THUMB mode. The ARM7TDMI manual says LR_irq should be set to the address of the next instruction + 4 (regardless of ARM/THUMB). The BIOS returns with `SUBS PC, LR, #4`, so:
- Wrong: LR = PC+2, return = PC-2 (lands 2 bytes before the correct instruction)
- Correct: LR = PC+4, return = PC (lands at the right instruction)

**The fix**: Always use `PC + 4` for the IRQ return address:

```rust
let return_addr = self.registers[R_PC].wrapping_add(4);
```

**The lesson**: The ARM pipeline means PC is always ahead of the executing instruction. For IRQ entry, the hardware saves PC+4 as LR regardless of instruction width. This is a fundamental ARM convention — don't try to be clever with different offsets per mode.

## Other Fixes in This Session

### LZ77 Decompression (SWI 0x11/0x12)

We implemented `LZ77UnCompWram` (SWI 0x11) and `LZ77UnCompVram` (SWI 0x12) — BIOS functions that decompress LZ77-compressed data. Most GBA games compress their graphics assets with LZ77 and call these functions to decompress them into VRAM.

The format uses a 4-byte header (type + decompressed size) followed by a stream of flag bytes and data. Each flag byte controls 8 blocks: bit=0 means literal byte, bit=1 means back-reference (copy from already-decompressed data).

### THUMB Format 5 ADD with Rd=PC

Same alignment issue as MOV — `ADD PC, Rs` in THUMB mode needs to force halfword alignment on the result.

## Current State: Wario Land 4 Boots!

After all four fixes, the game reaches its intro scene — a rendered blue corridor. The interrupt system works end-to-end: VBlank fires, BIOS handler runs, game's custom handler dispatches, flag is set, game proceeds. The remaining gaps for full gameplay:

1. **Sprites (OBJ layer)** — Wario himself is a sprite, not a background tile
2. **Timer system** — many games use timers for audio and animation
3. **More BIOS calls** — SWI 0x0F (ObjAffineSet), etc.
4. **Sound** — no audio system yet

## Key Takeaways

1. **Instruction decode priority matters** — when formats overlap in the encoding space, check the more specific pattern first. This bit us with SWI vs conditional branch.

2. **Every PC write needs alignment handling** — it's not just BX that modifies PC in THUMB mode. MOV, ADD, POP, LDM can all write to R15. Each needs to handle alignment correctly.

3. **Interrupts tie everything together** — without interrupts, games can't synchronize with the display hardware. The CPU, PPU, and I/O registers are all connected through the interrupt system.

4. **Real games stress-test everything at once** — a test ROM checks one subsystem; a real game needs CPU, memory, interrupts, BIOS, PPU, and DMA all working together. Testing incrementally with simpler ROMs is the right approach.

5. **The BIOS is more than a boot stub** — it provides runtime services (SWI calls) and handles interrupt dispatch. Either HLE it completely or execute the real BIOS code — mixing both is fragile.
