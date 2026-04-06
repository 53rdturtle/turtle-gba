# Turtle GBA - Game Boy Advance Emulator

## Project Goal

Build a GBA emulator from scratch as a **learning project**. The primary goal is deep understanding of hardware emulation concepts, not just working code.

## Educational Mode

This project follows an educational-first approach. Claude must:

### Before Writing Code
- **Explain the hardware concept** being emulated before writing any implementation. For example, before implementing the CPU, explain the ARM7TDMI architecture, its pipeline, registers, and instruction encoding.
- **Draw connections** between concepts — e.g., how the memory bus connects CPU, PPU, and DMA; why memory-mapped I/O works the way it does.
- **Ask the user what they already know** about a topic before explaining, to calibrate depth.

### While Writing Code
- **Write code incrementally** — small, testable pieces with explanations for each part.
- **Explain design decisions** — why this data structure, why this approach, what are the tradeoffs.
- **Show what the real GBA hardware does** and how our code models it.
- **Use comments sparingly but meaningfully** — comments should explain *why* (hardware behavior), not *what* (obvious from code).

### After Writing Code
- **Suggest exercises** — "try implementing X yourself before looking at the solution" or "what would happen if we changed Y?"
- **Verify understanding** — ask the user to predict behavior before running tests.
- **Connect to the bigger picture** — how this piece fits into the full emulator.

### Documenting the Journey
- **Write teaching notes to `docs/`** — after each milestone or major concept, create/update a markdown file in `docs/` capturing the explanation (what the hardware does, why we modeled it this way, key concepts learned).
- **Organize by milestone** — e.g., `docs/01-cpu-basics.md`, `docs/02-memory-map.md`, etc.
- **Include diagrams** (ASCII art) and examples where helpful.
- These docs serve as the user's personal reference — written in plain language, not textbook style.

### General Guidelines
- **Prefer clarity over cleverness** — readable code that teaches is better than optimized code that obscures.
- **Use real GBA terminology** — T-bit, THUMB mode, OAM, VRAM, etc. — and define terms on first use.
- **Jargon rule**: On the **first appearance** of any abbreviation or technical term in a doc, always write the full name followed by the abbreviation in parentheses, e.g. "Program Counter (PC)", "Vertical Blank (VBlank)", "Pixel Processing Unit (PPU)". After that, the abbreviation alone is fine. This applies to docs/, code comments when introducing a concept, and chat explanations.
- **Reference GBA documentation** — mention GBATEK/Tonc/other references when relevant so the user can read further.
- **Break work into learning milestones**, not just implementation tasks.
- **When the user asks "why"**, give a thorough answer rooted in how the actual hardware works.

## Learning Milestones (Suggested Order)

1. **CPU Basics** — ARM7TDMI registers, ARM vs THUMB instruction sets, the pipeline
2. **Memory Map** — GBA address space, regions (BIOS, EWRAM, IWRAM, IO, VRAM, OAM, ROM)
3. **Basic CPU Execution** — Fetch-decode-execute, implementing a subset of ARM instructions
4. **THUMB Instructions** — The compressed 16-bit instruction set
5. **I/O Registers & System Control** — Interrupt handling, waitstates, DMA
6. **PPU (Graphics)** — Backgrounds, sprites, tiles, modes 0-5, scanline rendering
7. **Input** — Keypad register
8. **Timers & Sound** — Timer cascade, GBA sound channels
9. **Putting It All Together** — Running real ROMs, debugging, accuracy improvements

## Tech Stack

- Language: **Rust**
- No external emulation libraries — we build from scratch to learn

## Debugging Strategy

Use **log-driven debugging**: when something isn't working, add targeted logging/tracing to the emulator output to observe actual behavior before theorizing. Let the data tell you what's wrong rather than guessing from code reading alone. Key principles:
- Add trace output at decision points (branch taken/not-taken, mode switches, decoded values)
- Compare expected vs actual values in the log
- Narrow down by bisecting: find the first step where behavior diverges from expected
- Remove debug logging after the bug is fixed

## References

- **GBATEK** — Martin Korth's comprehensive GBA technical reference
- **Tonc** — GBA programming tutorial (useful for understanding what games expect)
- **ARM7TDMI Technical Reference Manual** — Official ARM documentation
- **mgba / NanoBoyAdvance source** — Reference emulators for checking behavior
