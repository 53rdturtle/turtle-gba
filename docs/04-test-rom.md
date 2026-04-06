# Milestone 4: Test ROM & Execution Tracing

## GBA ROM Format

A GBA ROM (Read-Only Memory — the game cartridge data) is just a binary file (usually `.gba`) loaded at address `0x08000000` in the GBA's memory map. It has a header:

```
Offset  Size    Purpose
0x000   4       Branch instruction — jumps past the header to your code
0x004   156     Nintendo logo — BIOS checks this on real hardware
0x0A0   12      Game title (ASCII, like "POKEMON RUBY")
0x0B0   4       Game code
0x0B8   1       Fixed value: 0x96
0x0BE   1       Header checksum — BIOS verifies this
0x0C0+  ...     Your code starts here
```

The very first instruction at offset 0 is always a branch (`B start`) that jumps over the header. On real hardware the BIOS (Basic Input/Output System — the GBA's built-in startup code) checks the Nintendo logo and checksum — our emulator skips these checks.

## Our Test Program

We hand-crafted a ROM that computes 1+2+3+...+10 = 55:

```arm
entry:    B start           ; Jump past header
          ; ... header ...
start:    MOV R0, #0        ; counter = 0
          MOV R1, #10       ; limit
          MOV R2, #0        ; sum = 0
loop:     ADD R0, R0, #1    ; counter++
          ADD R2, R2, R0    ; sum += counter
          CMP R0, R1        ; done?
          BNE loop          ; if not, keep going
          STR R2, [R3]      ; store result to IWRAM (Internal Work RAM)
```

## Execution Trace

The trace shows each instruction as it executes with register values:

```
0x08000000  B +196          → jumps to 0x080000C0
0x080000C0  MOV R0, #0      R0=0
0x080000C4  MOV R1, #10     R1=10
0x080000C8  MOV R2, #0      R2=0
0x080000CC  ADD R0, R0, #1  R0=1        ← loop iteration 1
0x080000D0  ADD R2, R2, R0  R2=1
0x080000D4  CMP R0, R1      (1 != 10)
0x080000D8  BNE loop        → jumps back
... (repeats 10 times) ...
0x080000D0  ADD R2, R2, R0  R2=55       ← final sum!
0x080000D4  CMP R0, R1      (10 == 10, Z flag set)
0x080000D8  BNE loop        → NOT taken (Z flag blocks it)
0x080000E4  STR R2, [R3]    → writes 55 to IWRAM
```

## Key Learning: Conditional Branches in Practice

The loop works because:
1. `CMP R0, R1` subtracts R1 from R0 and sets flags (but doesn't store the result)
2. When R0 != R1, the Z flag is clear, so `BNE` (Branch if Not Equal) jumps back
3. When R0 == R1, the Z flag is set, so `BNE` falls through to the next instruction

This is the fundamental pattern for all loops in ARM assembly.

## Disassembler

We added a basic disassembler that converts hex instructions back to readable text. This is essential for debugging — reading `0xE2800001` is impossible, but reading `ADD R0, R0, #1` is clear.

## What's Next

Our CPU runs hand-crafted programs correctly. To run real GBA games we need:
- More instruction types (MUL = Multiply, LDM/STM = Load/Store Multiple registers, halfword loads, etc.)
- THUMB mode (the 16-bit compressed instruction set — see milestone 05)
- PPU (Pixel Processing Unit — the graphics rendering hardware)
- Interrupt handling (how the hardware notifies the CPU of events)
