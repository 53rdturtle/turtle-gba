# Milestone 7: PPU Rendering — Seeing Pixels on Screen

## What is the PPU?

The **PPU (Pixel Processing Unit)** is the GBA's dedicated graphics hardware. While the CPU runs game logic, the PPU's job is painting the screen — one horizontal line at a time, 60 times per second. The CPU tells the PPU *what* to draw by writing to VRAM (Video RAM), Palette RAM, and I/O registers. The PPU reads that data and produces pixels.

The GBA screen is **240 pixels wide x 160 pixels tall**.

## Display Modes

The GBA has 6 display modes, selected by bits 0-2 of the **DISPCNT (Display Control)** register at `0x04000000`:

```
Mode 0: Four tiled backgrounds (most common in games)
Mode 1: Two tiled + one affine (rotation/scaling) background
Mode 2: Two affine backgrounds
Mode 3: 16-bit bitmap (240x160, direct color — no palette)
Mode 4: 8-bit bitmap (240x160, 256-color palette, double-buffered)
Mode 5: 16-bit bitmap (160x128, smaller but double-buffered)
```

Armwrestler uses **Mode 4**, which is the simplest bitmap mode to render.

## How Mode 4 Works

In Mode 4, each pixel on screen is one byte in VRAM — an **index** into a 256-color palette:

```
  VRAM byte at offset (y * 240 + x) = palette index for pixel (x, y)
  Palette entry at index N = 2 bytes at Palette RAM offset (N * 2)
```

Each palette entry is a **15-bit BGR555** color packed into 16 bits:

```
  bits  0-4:  Red   (0-31)
  bits  5-9:  Green (0-31)
  bits 10-14: Blue  (0-31)
  bit  15:    unused
```

To convert BGR555 to the 8-bit RGB that a PC monitor understands, we scale each 5-bit channel to 8 bits: `(value << 3) | (value >> 2)`, which maps 0->0 and 31->255.

### Double Buffering

Mode 4 supports **double buffering** — two separate framebuffers in VRAM:
- **Frame 0**: VRAM offset `0x0000` (38,400 bytes)
- **Frame 1**: VRAM offset `0xA000` (38,400 bytes)

Bit 4 of DISPCNT selects which frame is displayed. Games draw to the hidden frame, then flip the bit — this prevents the player from seeing half-drawn frames (called "tearing").

## The Window

We use **minifb**, a minimal framebuffer library for Rust. It gives us a window and lets us write an array of 32-bit RGB pixels to it. Our render loop:

1. Run the CPU for one frame's worth of cycles (280,896 cycles = 228 scanlines x 1232 cycles)
2. Read VRAM and palette to produce a 240x160 pixel buffer
3. Push that buffer to the window
4. Repeat at ~60 FPS (frames per second)

We scale the window 4x (to 960x640) because 240x160 is very small on a modern monitor.

## Bug 6: LDR/STR Ignored the Byte Bit

**Symptom**: The screen showed scattered, garbled characters instead of readable text.

**What happened**: The ARM `LDR` (Load Register) / `STR` (Store Register) instruction has a **B bit** (bit 22) that selects between word and byte transfers:
- B=0: `LDR`/`STR` — load/store a 32-bit word (4 bytes)
- B=1: `LDRB`/`STRB` — load/store a single byte

Our `execute_single_transfer` function **ignored bit 22** and always did 32-bit word transfers. When armwrestler did `LDRB R4, [R0], #1` to read one character from its text string, our code loaded 4 bytes instead of 1. This meant R4 got four characters packed together, producing wrong glyph indices and completely garbled screen output.

**Fix**: Check bit 22 and use `read_byte`/`write_byte` when it's set:

```rust
let is_byte = (instruction >> 22) & 1 == 1;

if is_load {
    self.registers[rd] = if is_byte {
        bus.read_byte(effective_addr) as u32
    } else {
        bus.read_word(effective_addr)
    };
} else {
    if is_byte {
        bus.write_byte(effective_addr, self.registers[rd] as u8);
    } else {
        bus.write_word(effective_addr, self.registers[rd]);
    }
}
```

**Lesson**: This is the same pattern as Bug 1 (MSR ignoring SPSR bit) — a single bit in the instruction encoding completely changes the operation. Missing it produces output that *looks* like it's working (pixels appear, characters render) but the data is all wrong. The key debugging insight was noticing that R4 (the character being rendered) contained values like `0x4C41000C` instead of single-byte ASCII values like `0x41` ('A').

## What Armwrestler Shows

With the fix, armwrestler's "ALU TESTS PART 1" screen renders properly:

```
         ALU TESTS PART 1
  ADC                    OK
  AND                    OK
  BIC              BAD Z Rd
  CMN                    OK
  CMP              BAD CNZ Rd
  EOR                    OK
  MOV                    OK  (some fail)
  MVN              BAD CNRd
  RSC                    OK
  SBC              BAD C
  MUL                    OK
  MULL                   OK
  SMULL                  OK
```

- **Green "OK"** = our implementation passes that test
- **Red text** = our implementation has a bug for that instruction

The failing tests (BIC, CMP, MOV, MVN, SBC) all show flag-related errors ("BAD Z", "BAD CNZ", "BAD C") — meaning our ALU (Arithmetic Logic Unit) flag calculations are slightly wrong for those instructions. These are fixable bugs for a future milestone.

## Current State

The emulator now:
1. Opens a 960x640 window (240x160 scaled 4x)
2. Runs CPU instructions in real-time at ~60 FPS
3. Renders Mode 4 bitmap graphics from VRAM
4. Displays armwrestler's test results with readable text and color-coded pass/fail indicators
5. Supports `--headless` mode for testing without a window

## What's Next

- Fix the failing ALU flag tests (BIC, CMP, MOV, MVN, SBC flag handling)
- Add Mode 0/1 tiled background rendering (used by most games)
- Add sprite (OAM — Object Attribute Memory) rendering
- Add input handling (keypad) so we can navigate armwrestler's test pages
- DMA (Direct Memory Access) — hardware memory copy without CPU involvement
