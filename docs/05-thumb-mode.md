# Milestone 5: THUMB Mode, BX, MSR, and MUL

## Why Two Instruction Sets?

The ARM7TDMI supports two modes:
- **ARM mode**: 32-bit instructions — every instruction can do more, but takes 4 bytes
- **THUMB mode**: 16-bit instructions — simpler, but only 2 bytes each

GBA games overwhelmingly use THUMB because the ROM bus is only 16 bits wide. Reading a 32-bit ARM instruction from ROM takes **two bus cycles**, but a 16-bit THUMB instruction takes only **one**. So THUMB code runs nearly **twice as fast** from ROM despite each instruction doing less.

## Switching Modes: BX (Branch and Exchange)

The `BX Rm` instruction switches modes based on **bit 0** of the target address:
- Bit 0 = 1 → switch to THUMB, jump to address with bit 0 cleared
- Bit 0 = 0 → stay in ARM, jump to word-aligned address

The **T bit** (bit 5 of CPSR) tracks the current mode.

Common pattern in armwrestler:
```arm
ADD R0, PC, #1    ; R0 = PC + 8 + 1 (the +1 sets bit 0!)
BX R0             ; Jump to next instruction in THUMB mode
```

## The Pipeline Bug We Found

Running armwrestler immediately revealed that our PC was off by 4 bytes because the ARM7TDMI's 3-stage pipeline means PC reads as instruction_addr + 8 (ARM) or instruction_addr + 4 (THUMB). We added `read_reg()` to handle this transparently.

## MSR/MRS (Status Register Transfer)

- **MSR CPSR, Rm** — write a value to the status register (change mode, set flags)
- **MRS Rd, CPSR** — read the status register into a general register

Armwrestler uses MSR to switch between IRQ mode and System mode to set up separate stack pointers for each mode.

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
1. Initialized display registers (ARM)
2. Set up IRQ and System mode stack pointers (MSR)
3. Switched to THUMB mode (BX)
4. Cleared ~256KB of EWRAM in a tight THUMB loop (~196K instructions)
5. Switched back to ARM and tried to call a BIOS function → halt (we don't have BIOS)

**196,655 instructions executed successfully!**

## What's Still Missing

- **SWI** (Software Interrupt) — BIOS function calls
- **LDM/STM** in ARM mode — block data transfer
- **Halfword transfers** in ARM mode (LDRH/STRH/LDSB/LDSH)
- **PPU** (graphics) — armwrestler wants to display test results on screen
