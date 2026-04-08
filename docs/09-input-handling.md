# Milestone 9: Input Handling — Pressing Buttons

## How Real GBA Input Works

The GBA has 10 buttons: **A, B, Select, Start, Right, Left, Up, Down, R, L**. Unlike a modern game controller with analog sticks and pressure-sensitive triggers, these are simple digital switches — either pressed or not.

The hardware exposes button state through a single register:

### KEYINPUT Register (0x04000130)

```
Bit  0: A            Bit  5: Left
Bit  1: B            Bit  6: Up
Bit  2: Select       Bit  7: Down
Bit  3: Start        Bit  8: R shoulder
Bit  4: Right        Bit  9: L shoulder
Bits 10-15: unused (read as 0)
```

### Active-Low: Why 0 Means "Pressed"

KEYINPUT is **active-low** — a bit is **0** when the button IS pressed and **1** when it's NOT pressed. When nothing is pressed, the register reads `0x03FF` (all 10 bits set). This seems backwards, but it comes from how the electrical circuit works:

```
Button released:       Button pressed:
    VCC (3.3V)             VCC (3.3V)
     |                      |
     R (pull-up)            R (pull-up)
     |                      |
     +--- signal = HIGH     +--- signal = LOW
     |                      |
   [open switch]          [closed switch]
     |                      |
    GND                    GND
```

Each button is a switch between the signal line and ground. A **pull-up resistor** keeps the line at high voltage (logic 1) when the switch is open. Pressing the button closes the switch, connecting the line to ground (logic 0). The register just reflects this electrical state directly — no inversion.

This "active-low" convention is extremely common in hardware. It's simpler and more reliable than the alternative because:
- The default (unpressed) state is well-defined (pulled high)
- No current flows when buttons aren't pressed (saves power)
- Ground is a stronger, more stable reference than VCC

### KEYCNT Register (0x04000132)

There's also a **KEYCNT** (Key Control) register that can generate an interrupt when specific button combinations are pressed. We don't implement this yet — armwrestler doesn't need it. Games that use it typically want "press A+B+Select+Start simultaneously to soft-reset."

## How Games Read Input

Most GBA games read input during the **VBlank** (Vertical Blank) period — the ~4.6ms gap after the PPU finishes drawing the screen and before it starts the next frame. This gives a consistent 60Hz polling rate.

A typical game input routine looks like:

```c
// Read KEYINPUT and invert so 1 = pressed
u16 keys = ~(*(volatile u16*)0x04000130) & 0x03FF;

// Now test individual buttons naturally
if (keys & KEY_A)     { /* A pressed */ }
if (keys & KEY_RIGHT) { /* moving right */ }
```

Games invert the bits so they can use natural `if (pressed)` logic. Some games also track edges (newly pressed vs held) by comparing with the previous frame's state:

```c
u16 keys_new = keys & ~keys_prev;  // Just pressed this frame
u16 keys_released = ~keys & keys_prev;  // Just released
```

Armwrestler uses this to detect Left/Right presses for page navigation — it only moves the cursor on the frame where the button transitions from released to pressed, not while it's held.

## Our Implementation

### The Bus Side

We added a `keyinput` field to the `Bus` struct, defaulting to `0x03FF`:

```rust
pub struct Bus {
    // ...
    pub keyinput: u16,  // Active-low: 0x03FF = all released
}
```

Reads from I/O offset `0x130`/`0x131` return this field instead of the generic I/O array:

```rust
0x0400_0000..=0x0400_03FE => {
    let offset = (addr & 0x3FF) as usize;
    match offset {
        0x130 => self.keyinput as u8,        // KEYINPUT low byte
        0x131 => (self.keyinput >> 8) as u8,  // KEYINPUT high byte
        _ => self.io[offset],
    }
}
```

This is important: KEYINPUT is **read-only** from the CPU's perspective. Games can't "press buttons" by writing to this register — only the physical hardware (or our emulator's input layer) sets it.

### The Window Side

Each frame, we poll minifb's keyboard state and map PC keys to GBA buttons:

```rust
fn update_keyinput(window: &Window, bus: &mut Bus) {
    let key_map: &[(Key, u16)] = &[
        (Key::Z,         1 << 0),  // A
        (Key::X,         1 << 1),  // B
        (Key::Backspace, 1 << 2),  // Select
        (Key::Enter,     1 << 3),  // Start
        (Key::Right,     1 << 4),  // Right
        (Key::Left,      1 << 5),  // Left
        (Key::Up,        1 << 6),  // Up
        (Key::Down,      1 << 7),  // Down
        (Key::S,         1 << 8),  // R shoulder
        (Key::A,         1 << 9),  // L shoulder
    ];

    let mut keyinput: u16 = 0x03FF;
    for &(key, bit) in key_map {
        if window.is_key_down(key) {
            keyinput &= !bit;  // Clear bit = pressed (active-low)
        }
    }
    bus.keyinput = keyinput;
}
```

### The Polling Timing Lesson

We initially ran 10 GBA frames per window update (`FRAMES_PER_RENDER = 10`) for speed. This caused ~0.5 second input lag because **minifb only refreshes key state when `window.update()` is called**. Between updates, `is_key_down()` returns stale data from the last update.

We confirmed this with logging — key presses only appeared immediately after `update_with_buffer()`, never during the inner frame loop. The fix was setting `FRAMES_PER_RENDER = 1`: render every GBA frame so input is polled at the full 60Hz.

```
Before: poll keys → run 10 frames → render → poll keys (167ms between polls)
After:  poll keys → run 1 frame → render → poll keys (16ms between polls)
```

This is a general emulator design lesson: **never batch input polling**. You can skip rendering frames for speed, but the emulation must see input changes at the correct rate. Professional emulators handle this by separating the render rate from the emulation rate — they might render at 30fps but still poll input and run emulation at 60fps.

## Key Mapping Reference

```
PC Keyboard          GBA Button
───────────          ──────────
Z                    A
X                    B
Arrow keys           D-pad (Up/Down/Left/Right)
Enter                Start
Backspace            Select
A                    L shoulder
S                    R shoulder
Escape               Close emulator
```

## Current State

The emulator now:
1. Opens a 960x640 window with real-time rendering at ~60 FPS
2. Accepts keyboard input mapped to all 10 GBA buttons
3. Responds to input within one frame (~16ms)
4. Can navigate armwrestler's test pages to view all CPU test results

## What's Next

- Explore armwrestler's other test pages (ALU part 2, multiply, memory, THUMB)
- Fix any new test failures discovered on those pages
- Implement DMA (Direct Memory Access) — hardware memory copying
- Add Mode 0 tiled background rendering for real games
- Timers — needed for sound and game timing
