# The Big Picture: What Are We Building?

## What is an Emulator?

A GBA (Game Boy Advance) is a tiny computer. It has:
- A **CPU (Central Processing Unit)** — the brain that executes instructions one by one
- **Memory** — stores the game, variables, graphics data
- A **screen** — 160x240 pixels
- **Buttons** — A, B, D-pad, etc.

An emulator is a program that **pretends to be that computer**. Our Rust program will:
1. Read a GBA game file (the ROM — Read-Only Memory, i.e. the game cartridge data)
2. Execute instructions one at a time, just like the real CPU would
3. Draw pixels to a window, just like the real screen would
4. Read keyboard input, just like the real buttons would

That's it. Everything else is details.

## What is a CPU?

Think of a CPU as a calculator that follows a recipe. The recipe (the game) is a long list of very simple instructions like:
- "Put the number 5 into box A"
- "Add box A and box B, store result in box C"
- "If box C is zero, skip ahead 10 steps"

The "boxes" are called **registers** — tiny, fast storage slots inside the CPU. The GBA's CPU is called the **ARM7TDMI** (a chip designed by ARM Ltd. — the name stands for ARM architecture v7, Thumb, Debug, Multiplier, ICE). It has 16 registers.

The instructions are stored as **numbers in memory**. The CPU reads them one at a time:
1. **Fetch** — read the next instruction from memory
2. **Decode** — figure out what the instruction means
3. **Execute** — do it

Repeat forever. That loop is the heartbeat of the emulator.

## How the Pieces Connect

```
┌──────────────────────────────────────┐
│              GBA System              │
│                                      │
│  ┌─────────┐      ┌──────────────┐  │
│  │  CPU     │◄────►│   Memory     │  │
│  │ ARM7TDMI│      │  (Bus)       │  │
│  └─────────┘      └──────┬───────┘  │
│                          │           │
│       ┌──────────┬───────┼────────┐  │
│       ▼          ▼       ▼        ▼  │
│   ┌──────┐  ┌──────┐ ┌─────┐ ┌────┐│
│   │ BIOS │  │ VRAM │ │ ROM │ │ IO ││
│   └──────┘  └──────┘ └─────┘ └────┘│
│                 │                │   │
│                 ▼                ▼   │
│            ┌────────┐     ┌───────┐ │
│            │ Screen │     │Keypad │ │
│            │160x240 │     │A B L R│ │
│            └────────┘     └───────┘ │
└──────────────────────────────────────┘
```

The CPU talks to everything through the **memory bus** — a shared address system (like a highway with numbered exits). When the CPU reads address `0x08000000`, it gets game ROM data. When it writes to address `0x04000000`, it changes a hardware setting. This is called **memory-mapped I/O (Input/Output)** — hardware is controlled by reading/writing specific memory addresses, as if the hardware registers were just locations in memory.

**BIOS** = Basic Input/Output System (built-in startup code). **VRAM** = Video RAM (Random Access Memory) — stores graphics. **IO** = Input/Output registers that control hardware.

## Next Up

We'll set up the Rust project and build the skeleton: a CPU struct with 16 registers and a simple memory system.
