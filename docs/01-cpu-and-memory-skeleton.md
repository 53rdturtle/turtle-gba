# Milestone 1: CPU & Memory Skeleton

## What We Built

Two core modules that model the GBA's brain (CPU) and its address space (memory bus).

## Key Concepts

### Registers (cpu.rs)

The ARM7TDMI CPU has **16 registers**, each a 32-bit number (`u32` in Rust):

| Register | Name | Purpose |
|----------|------|---------|
| R0-R12 | General purpose | Store whatever the program needs |
| R13 | **SP (Stack Pointer)** | Tracks the top of the stack (a region of memory used for temporary data) |
| R14 | **LR (Link Register)** | Stores the return address after function calls (so the CPU knows where to go back) |
| R15 | **PC (Program Counter)** | Points to the **next instruction** to execute — the CPU's "current position" in the program |

The **CPSR (Current Program Status Register)** holds condition flags — single bits that describe the result of the last operation:
- **N** (bit 31) — result was Negative
- **Z** (bit 30) — result was Zero
- **C** (bit 29) — Carry occurred
- **V** (bit 28) — signed Overflow occurred

These flags let the CPU make decisions: "if zero, jump here."

### Bit Manipulation

Flags are single bits inside a 32-bit number. We use bitwise operations:
- `cpsr | mask` — turn a bit ON (OR)
- `cpsr & !mask` — turn a bit OFF (AND with inverted mask)
- `cpsr & mask != 0` — check if a bit is ON

This is **fundamental to emulation** — real hardware is all bits.

### Memory Map (bus.rs)

The GBA's address space is 32-bit (addresses 0x00000000 to 0xFFFFFFFF), but only certain ranges are used:

```
0x00000000  BIOS       (16 KB)  — Basic Input/Output System, built-in startup code, read-only
0x02000000  EWRAM      (256 KB) — External Work RAM (Random Access Memory), general purpose, slower
0x03000000  IWRAM      (32 KB)  — Internal Work RAM, fast (on the CPU chip itself)
0x04000000  I/O Regs   (1 KB)   — Input/Output Registers, control hardware by writing here
0x05000000  Palette    (1 KB)   — color data for graphics (which colors the screen can use)
0x06000000  VRAM       (96 KB)  — Video RAM, stores graphics tile/bitmap data
0x07000000  OAM        (1 KB)   — Object Attribute Memory, stores sprite positions and properties
0x08000000  ROM        (≤32 MB) — Read-Only Memory, the game cartridge data
```

### Memory-Mapped I/O

The GBA doesn't have special CPU instructions for controlling hardware. Instead, specific memory addresses *are* the hardware controls. Writing to address `0x04000000` changes the display mode. This is called **memory-mapped I/O**.

### Little-Endian Byte Order

The GBA stores multi-byte numbers with the **least significant byte first**:
- The number `0xDEADBEEF` is stored as bytes: `EF BE AD DE`
- Address+0 = `0xEF` (smallest part), Address+3 = `0xDE` (biggest part)

This is called **little-endian** (the "little end" comes first).

### Memory Mirroring

Some regions repeat (mirror) throughout their address range. For example, IWRAM is 32 KB but mapped to a 16 MB range (0x03000000-0x03FFFFFF). Address 0x03008000 accesses the same byte as 0x03000000. We handle this with bitwise AND masking: `addr & 0x7FFF` keeps only the lower 15 bits (32 KB range).

## Files

- `src/cpu.rs` — CPU state: 16 registers, CPSR flags, flag helpers
- `src/bus.rs` — Memory bus: region routing, read/write byte/word/halfword
- `src/main.rs` — Entry point, wires CPU and bus together

## What's Next

The CPU can hold state and memory can store data — but nothing *executes* yet. Next we'll implement the **fetch-decode-execute loop**: the CPU reads an instruction from memory, figures out what it means, and does it.
