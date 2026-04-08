# Milestone 11: DMA — Direct Memory Access

With the CPU passing all armwrestler tests, the next hardware subsystem to tackle is DMA (Direct Memory Access). DMA is the GBA's hardware memory-copy engine — it moves data between memory regions without the CPU executing load/store instructions for every word. Nearly every commercial GBA game uses DMA, most commonly to fill or copy VRAM during VBlank.

## Why DMA Exists

Consider filling the screen with a solid color. The GBA's display is 240x160 pixels at 16 bits per pixel = 38,400 halfwords = 76,800 bytes. Using the CPU, that's roughly 38,400 LDR/STR cycles — the processor is busy doing nothing but shuttling identical values. DMA offloads this: the CPU writes a source, destination, and count to a few I/O registers, flips an enable bit, and the hardware does the rest in the background (or rather, by pausing the CPU briefly while the DMA controller takes over the bus).

Real games use DMA for:
- **Screen clears and fills** — writing a constant value across VRAM
- **Sprite/tile uploads** — copying tile graphics from ROM into VRAM
- **Double buffering** — copying the back buffer to the front buffer during VBlank
- **Sound streaming** — DMA1/DMA2 feed the FIFO sound channels with audio samples on a timer
- **HBlank effects** — updating background scroll registers every scanline for wave/wobble effects

## The Four DMA Channels

The GBA has four DMA channels (DMA0–DMA3), each with identical register layouts but different capabilities and priorities:

```
Channel   I/O Base    Max Count   Source Range    Dest Range     Special Use
───────   ────────    ─────────   ────────────    ──────────     ───────────
DMA0      0x040_00B0  0x4000      Internal only   Internal only  Highest priority
DMA1      0x040_00BC  0x4000      Any             Internal only  Sound FIFO A
DMA2      0x040_00C8  0x4000      Any             Internal only  Sound FIFO B
DMA3      0x040_00D4  0x10000     Any             Any            General purpose / Game Pak
```

"Internal" means addresses 0x00000000–0x07FFFFFF (BIOS through VRAM). "Any" includes the Game Pak ROM region (0x08000000+). DMA3 is the most flexible and the one games use most for general-purpose copies.

**Priority**: When multiple channels trigger simultaneously, lower-numbered channels go first. DMA0 has the highest priority and preempts the others.

## Register Layout

Each channel has 12 bytes of registers:

```
Offset +0: SAD (Source Address)       — 32-bit, write-only
Offset +4: DAD (Destination Address)  — 32-bit, write-only
Offset +8: CNT_L (Word Count)        — 16-bit, write-only
Offset +A: CNT_H (Control)           — 16-bit, read/write
```

### CNT_H — The Control Register

```
Bits    Name              Description
─────   ────              ───────────
5-6     Dest Control      0=Increment, 1=Decrement, 2=Fixed, 3=Increment/Reload
7-8     Source Control    0=Increment, 1=Decrement, 2=Fixed, 3=Prohibited
9       Repeat            1=Restart transfer on each trigger (VBlank/HBlank)
10      Transfer Type     0=16-bit, 1=32-bit
12-13   Start Timing      0=Immediately, 1=VBlank, 2=HBlank, 3=Special
14      IRQ on finish     1=Trigger interrupt when transfer completes
15      Enable            1=Channel active (writing 1 starts/arms the transfer)
```

### Address Control Modes

The source and destination addresses can be adjusted after each unit is transferred:

- **Increment (0)**: Address increases by 2 (16-bit) or 4 (32-bit) after each transfer. This is the normal "copy forward" mode.
- **Decrement (1)**: Address decreases. Used for copying data backwards.
- **Fixed (2)**: Address stays the same. Useful for reading from a single I/O register or writing a constant (e.g., filling memory with a fixed value by reading from a single address that holds that value).
- **Increment/Reload (3, dest only)**: Same as increment during a transfer, but the destination address resets to the original value before each repeat trigger. Used for HBlank effects where you write to the same register every scanline.

## How a DMA Transfer Works

### Step 1: Latching

When the CPU writes to CNT_H and the enable bit transitions from 0 to 1 (rising edge), the DMA controller **latches** (snapshots) the source address, destination address, and word count into internal registers. This is important: the CPU can overwrite the I/O registers afterward without affecting an in-progress or armed transfer.

```rust
if !was_enabled && self.dma[ch].enabled() {
    self.dma[ch].internal_sad = sad & sad_mask;
    self.dma[ch].internal_dad = dad & dad_mask;
    // ...
}
```

The address masks enforce hardware limits: DMA0 can only source from 27-bit addresses (internal memory), while DMA3 can source from 28-bit addresses (including Game Pak).

### Step 2: Transfer

For immediate timing (start_timing = 0), the transfer runs right away. For VBlank/HBlank timing, the channel is "armed" and waits until the PPU signals the appropriate blanking period.

During transfer, the DMA controller takes over the bus. The CPU is paused — it cannot fetch instructions or access memory until the transfer completes. For each unit:

1. Read a halfword or word from the source address
2. Write it to the destination address
3. Adjust source and destination by the step size per their control modes

```rust
for _ in 0..count {
    if transfer_32 {
        let val = self.read_word(src);
        self.write_word(dst, val);
    } else {
        let val = self.read_halfword(src);
        self.write_halfword(dst, val);
    }
    // adjust src/dst per control modes...
}
```

### Step 3: Completion

After all units are transferred:

- If the **repeat** bit is set AND timing is VBlank/HBlank: the channel stays enabled and will fire again on the next trigger. The destination address reloads if dest_control = 3 (increment/reload).
- Otherwise: the enable bit is cleared automatically. The transfer is done.

```rust
if !self.dma[ch].repeat() || self.dma[ch].start_timing() == 0 {
    self.dma[ch].control &= !(1 << 15); // Clear enable
}
```

### Word Count = 0 Means Maximum

A special case: if the CPU writes 0 to CNT_L, it doesn't mean "transfer nothing." It means "transfer the maximum number of units" — 0x4000 for DMA0–2, or 0x10000 for DMA3. This is a common pattern in hardware: a zero in a count field wraps to the maximum because the counter is a fixed-width integer.

## Timing Triggers

### Immediate (0)
Transfer starts the instant CNT_H is written with the enable bit set. Used for one-shot copies like loading tile data into VRAM.

### VBlank (1)
Transfer triggers at the start of every Vertical Blanking period (scanline 160). This is the safe window to update VRAM without visual artifacts, since the PPU isn't reading it for display. Games use this to upload the next frame's graphics.

### HBlank (2)
Transfer triggers at the end of every visible scanline (during the Horizontal Blanking period). Combined with repeat + increment/reload, this enables per-scanline effects: write a new scroll value to a background register 160 times per frame to create wave/ripple distortion.

### Special (3)
Channel-specific behavior:
- **DMA1/DMA2**: Sound FIFO refill — triggered when the audio FIFO needs more samples
- **DMA3**: Video capture (rarely used)

We haven't implemented Special timing yet since it requires the sound system.

## Integration with the PPU

DMA triggers are detected using **rising edge detection** in the PPU timing loop. We compare the previous DISPSTAT value with the current one to find the exact moment VBlank or HBlank begins:

```rust
let was_vblank = old_dispstat & 0x01 != 0;
if in_vblank && !was_vblank { self.check_vblank_dma(); }

let was_hblank = old_dispstat & 0x02 != 0;
if in_hblank && !was_hblank { self.check_hblank_dma(); }
```

Rising edge detection is critical: without it, a VBlank DMA with repeat enabled would fire continuously throughout VBlank (scanlines 160–227 = 68 times!) instead of once at the start.

## What We Haven't Implemented Yet

- **DMA priority/preemption**: When multiple channels trigger, they should execute in priority order and higher-priority channels should be able to preempt lower ones mid-transfer
- **Bus cycle timing**: Our transfers are instant. Real DMA takes 2 cycles per unit plus setup overhead, and the CPU is genuinely stalled during this time
- **DMA IRQ**: The interrupt-on-finish bit (bit 14) isn't wired up yet — needs the interrupt controller
- **Special timing (sound FIFO)**: Needs the sound system first
- **Open bus behavior**: Reading from invalid DMA source addresses has specific behavior on real hardware

## Lessons Learned

1. **Rising edge detection is a recurring pattern**: just like button presses, DMA triggers need edge detection, not level detection. Any time hardware says "do X when Y becomes true," you need to track the previous state and compare.

2. **Latching separates configuration from execution**: the CPU writes to I/O registers at its own pace, but the DMA controller snapshots those values at enable time. This is a common hardware pattern — it prevents race conditions where the CPU modifies registers mid-transfer.

3. **Zero means maximum, not zero**: hardware count registers that wrap around are everywhere. When you see a count field in hardware documentation, always check what count=0 means.
