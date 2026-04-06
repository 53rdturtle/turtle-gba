# Milestone 5: THUMB Mode, BX, MSR, and MUL

## Why Two Instruction Sets?

The ARM7TDMI CPU supports two instruction modes:
- **ARM mode**: 32-bit instructions — every instruction can do more, but takes 4 bytes of memory
- **THUMB mode**: 16-bit instructions — simpler and less powerful per instruction, but only 2 bytes each

GBA games overwhelmingly use THUMB because the ROM (game cartridge) bus is only 16 bits wide. A "bus" is the data highway between the CPU and memory — its width determines how many bits can be transferred at once. Reading a 32-bit ARM instruction from ROM takes **two bus cycles** (two trips on the 16-bit highway), but a 16-bit THUMB instruction takes only **one trip**. So THUMB code runs nearly **twice as fast** from ROM despite each instruction doing less.

## Switching Modes: BX (Branch and eXchange)

The `BX Rm` instruction switches between ARM and THUMB mode based on **bit 0** of the target address:
- Bit 0 = 1 → switch to THUMB, jump to address with bit 0 cleared
- Bit 0 = 0 → stay in ARM, jump to word-aligned address

The **T bit** (bit 5 of the CPSR — Current Program Status Register) tracks which mode the CPU is currently in: 0 = ARM, 1 = THUMB.

Common pattern in armwrestler:
```arm
ADD R0, PC, #1    ; R0 = PC + 8 + 1 (the +1 sets bit 0!)
BX R0             ; Jump to next instruction in THUMB mode
```

## The Pipeline Bug We Found

Running armwrestler (a test ROM that exercises ARM/THUMB instructions) immediately revealed that our PC (Program Counter) was off by 4 bytes. This is because the ARM7TDMI has a **3-stage pipeline** — it fetches, decodes, and executes instructions in an overlapping assembly line. By the time an instruction executes, the PC has already moved ahead to fetch future instructions: PC reads as `instruction_address + 8` in ARM mode or `instruction_address + 4` in THUMB mode. We added a `read_reg()` helper to handle this transparently.

## MSR/MRS (Status Register Transfer)

These instructions move data between general registers and the CPSR (Current Program Status Register):

- **MSR (Move to Status Register)**: `MSR CPSR, Rm` — write a value to the status register (change processor mode, set flags)
- **MRS (Move from Status Register)**: `MRS Rd, CPSR` — read the status register into a general register

Armwrestler uses MSR to switch between IRQ (Interrupt Request) mode and System mode to set up separate stack pointers for each processor mode.

## THUMB Instruction Formats

THUMB has ~19 formats, identified by the top bits. We implemented:

| Format | Top bits | Instructions | Purpose |
|--------|----------|-------------|---------|
| 1 | 000xx | LSL, LSR, ASR | Shift register by immediate |
| 2 | 00011 | ADD, SUB | Add/subtract register or small immediate |
| 3 | 001xx | MOV, CMP, ADD, SUB | 8-bit immediate operations |
| 4 | 01000-0 | AND, EOR, ADC, ... | 16 ALU operations on low registers |
| 5 | 01000-1 | ADD, CMP, MOV, BX | Hi register ops (access R8-R15) |
| 6 | 01001 | LDR Rd, [PC, #] | PC-relative load |
| 7/8 | 0101x | LDR/STR with Ro | Register-offset load/store |
| 9 | 011xx | LDR/STR with #imm | Immediate-offset load/store |
| 10 | 1000x | LDRH/STRH | Halfword load/store |
| 11 | 1001x | LDR/STR [SP, #] | SP-relative load/store |
| 12 | 1010x | ADD Rd, PC/SP, # | Load address |
| 13 | 10110000 | ADD SP, # | Adjust stack pointer |
| 14 | 1011x10x | PUSH/POP | Push/pop registers to/from stack |
| 15 | 1100x | LDMIA/STMIA | Multiple load/store |
| 16 | 1101xxxx | Bcc | Conditional branch |
| 18 | 11100 | B | Unconditional branch |
| 19 | 1111x | BL (two parts) | Long branch with link (function call) |

### Key difference from ARM:
- THUMB can only directly access **R0-R7** (the "low registers")
- Format 5 is the exception — it can access R8-R15 for ADD, MOV, CMP, and BX
- **All THUMB ALU ops set flags** (no S bit — it's always on)

## Armwrestler Results

With THUMB mode implemented, armwrestler:
1. Initialized display registers (in ARM mode)
2. Set up IRQ (Interrupt Request) and System mode stack pointers (using MSR)
3. Switched to THUMB mode (using BX)
4. Cleared ~256KB of EWRAM (External Work RAM) in a tight THUMB loop (~196K instructions)
5. Switched back to ARM and tried to call a BIOS (Basic Input/Output System) function → halt (we didn't have BIOS yet)

**196,655 instructions executed successfully!**

## What's Still Missing

- **SWI (Software Interrupt)** — how games call built-in BIOS functions
- **LDM/STM (Load/Store Multiple)** in ARM mode — transfer many registers to/from memory at once
- **Halfword transfers** in ARM mode (LDRH/STRH/LDRSB/LDRSH — load/store 16-bit and signed values)
- **PPU (Pixel Processing Unit)** — the graphics hardware; armwrestler wants to display test results on screen
