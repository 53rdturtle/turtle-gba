# Milestone 15: Timing Accuracy and Alpha Blending

Two issues made the emulator feel wrong: the game ran 2-3x too fast, and transparency effects were missing. Both required understanding how the GBA's hardware timing and compositing work.

## Problem 1: Game Runs Too Fast

The game appeared to run at the correct 60 FPS (verified via FPS counter), yet animations and gameplay were clearly 2-3x faster than real hardware. This paradox has two causes.

### Cause A: No Memory Waitstates

The GBA's CPU runs at 16.78 MHz, but not all memory is equally fast. The ARM7TDMI accesses different memory regions through buses with different widths and speeds:

```
Region        Bus Width   Access Time   Notes
---------     ---------   -----------   -----
BIOS          32-bit      1 cycle       On-chip, fast
IWRAM         32-bit      1 cycle       On-chip, fast  
EWRAM         16-bit      2-3 cycles    Off-chip, 16-bit bus
ROM (Game Pak) 16-bit     2-8 cycles    Off-chip, configurable via WAITCNT
```

ROM access is particularly important because most game code lives there. The WAITCNT register (0x04000204) configures ROM access timing:

```
WAITCNT bits:
  2-3:  WS0 non-sequential wait (0=4, 1=3, 2=2, 3=8 cycles)
  4:    WS0 sequential wait (0=2, 1=1 cycles)
  14:   Prefetch buffer enable
```

Wario Land 4 sets WAITCNT to 0x45B4, which means:
- WS0 non-sequential: 3 wait cycles (after a branch)
- WS0 sequential: 1 wait cycle (consecutive fetches)
- Prefetch buffer: enabled

**The prefetch buffer** is a small 8-byte FIFO that fetches the next ROM halfword while the CPU is busy executing. For sequential THUMB code (the common case), the next instruction is already prefetched, so the fetch appears "free." Branches break the prefetch pipeline and incur the full non-sequential cost.

Our fix: `fetch_waitstates()` checks the PC's memory region and adds the appropriate wait cycles to each instruction's cost. THUMB from ROM costs +2 cycles, ARM from ROM costs +4 (32-bit fetch needs two 16-bit bus accesses).

### Cause B: BIOS Halt Desync (The Real Bug)

Even with waitstates, the game was still too fast. The root cause was a **timing desync** between the CPU and the main loop.

The main loop tracked time with its own `total_cycles` counter:

```rust
while total_cycles < CYCLES_PER_FRAME {
    let cycles = cpu.step(bus);  // Returns 3 for SWI
    bus.tick(cycles);
    total_cycles += cycles;
}
```

But BIOS Halt (SWI 0x02) and VBlankIntrWait (SWI 0x05) advance `bus.cycles` internally:

```rust
// Inside handle_bios_call:
0x02 => {
    // Halt — tick bus until interrupt fires
    for _ in 0..280_896 {
        bus.tick(1);         // Advances bus.cycles by thousands
        if bus.irq_pending() { break; }
    }
}
```

After Halt returns, `cpu.step()` returns only 3 cycles (the SWI execution cost). The main loop adds 3 to `total_cycles`, thinking the frame is barely started. But `bus.cycles` has already jumped past the VBlank boundary. The game then gets an entire frame's worth of additional CPU time, running its game logic 2-3x per display frame.

**The fix**: use `bus.cycles` as the single source of truth:

```rust
let frame_start = bus.cycles;
while bus.cycles.wrapping_sub(frame_start) < CYCLES_PER_FRAME {
    let cycles = cpu.step(bus);
    if cycles == 0 { break; }
    bus.tick(cycles);
}
```

Now when Halt advances `bus.cycles` internally, the frame boundary check sees it immediately. No desync.

**The lesson**: when multiple components advance the same logical clock, use one authoritative counter. Maintaining separate counters that must stay in sync is fragile — any code path that advances one but not the other creates a timing bug.

## Problem 2: Missing Transparency/Blending

### The GBA's Compositing Pipeline

The GBA doesn't just draw layers on top of each other. It has a hardware compositing unit that processes pixels through a priority-based pipeline with optional blending effects.

**Layer priority**: Each background has a 2-bit priority field (0=highest, 3=lowest) in its BGxCNT register. Each sprite has its own 2-bit priority in OAM Attr2. At each pixel, the hardware finds the two highest-priority opaque layers — the "top" and "second" layer.

Priority ordering (highest to lowest):
1. OBJ at priority 0
2. BG0 at priority 0
3. BG1 at priority 0
4. OBJ at priority 1
5. BG0 at priority 1
... and so on through priority 3, then the backdrop (palette[0]).

This interleaving is important — a priority-2 sprite draws *behind* a priority-1 background, not on top of it. Our original code drew all sprites on top of all backgrounds, which was wrong.

### Blend Control (BLDCNT — 0x04000050)

The GBA supports three blend effects, selected by bits 6-7 of BLDCNT:

```
Mode 0: None — top layer drawn as-is
Mode 1: Alpha blend — top and second layer mixed
Mode 2: Brightness increase — top layer fades toward white
Mode 3: Brightness decrease — top layer fades toward black
```

BLDCNT also specifies which layers are "first targets" (bits 0-5) and "second targets" (bits 8-13). Blending only occurs when the top layer is a first target AND the second layer is a second target.

**Alpha blending** (BLDALPHA at 0x04000052):
```
EVA (bits 0-4): First target coefficient (0-16)
EVB (bits 8-12): Second target coefficient (0-16)

result = (first * EVA + second * EVB) / 16
```

This creates semi-transparent effects — windows you can see through, ghostly enemies, water surfaces.

**Brightness** (BLDY at 0x04000054):
```
EVY (bits 0-4): Brightness coefficient (0-16)

Increase: color + (white - color) * EVY / 16
Decrease: color - color * EVY / 16
```

Games use this for fade-in (brightness increase from 16→0), fade-out (brightness decrease from 0→16), and flash effects.

### Implementation

We restructured the PPU to:
1. Pre-render each BG and OBJ layer into separate pixel buffers
2. Composite per-pixel: find the top two opaque layers by priority
3. Apply the blend effect based on BLDCNT/BLDALPHA/BLDY

This is more work per frame than the old approach (which just painted layers on top of each other), but it produces correct results for games that rely on blending for their visual effects.

## Key Takeaways

1. **One clock, one counter** — when BIOS calls advance the system clock, the main loop must see it. Separate counters for the same concept are a bug waiting to happen.

2. **Memory speed is not uniform** — the GBA's bus architecture means ROM access can cost 2-8x more than IWRAM access. Ignoring this makes everything run too fast.

3. **The prefetch buffer changes everything** — with prefetch enabled, sequential ROM code runs nearly as fast as IWRAM code. This is why GBA games can run primarily from ROM without being unplayably slow.

4. **Compositing is more than painter's algorithm** — correct GBA rendering requires tracking per-pixel priority across all layers and applying blend effects conditionally based on which layers are first/second targets.
