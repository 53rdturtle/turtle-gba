# Milestone 17: Hardware Timers

The GBA has 4 hardware timers (TM0-TM3) that count upward and fire interrupts on overflow. They're essential for audio timing, animation pacing, and general-purpose measurement.

## What Timers Do

A timer is a 16-bit counter that increments at a configurable rate. When it passes 0xFFFF, it **overflows**: the counter reloads to a preset value, and optionally fires an interrupt. This creates a repeating cycle at a precise frequency.

For example, to play audio at 16,384 Hz (a common GBA sample rate):
- Set timer reload = 0x10000 - (16,780,000 / 16,384) = 0xFBFF
- Timer counts from 0xFBFF up to 0xFFFF (1025 cycles), overflows, reloads, repeat
- Each overflow triggers DMA to send the next audio sample

## The Registers

Each timer has two addresses — one shared for counter/reload, one for control:

```
Timer   Counter/Reload   Control
TM0     0x04000100       0x04000102
TM1     0x04000104       0x04000106
TM2     0x04000108       0x0400010A
TM3     0x0400010C       0x0400010E
```

**Reading** the counter address gives the live count. **Writing** to the same address sets the **reload value** — the starting point after each overflow. These are different operations at the same address, which is unusual.

### Control Register

```
Bit 7:    Enable (0 = stopped, 1 = running)
Bit 6:    IRQ on overflow (fires IF bit 3+N)
Bit 2:    Cascade mode
Bits 0-1: Prescaler select
```

### Prescaler — Counting Speed

The prescaler divides the 16.78 MHz system clock:

```
Value   Divider   Effective Rate    Use Case
0       1         16.78 MHz         High-res timing
1       64        262 KHz           General purpose
2       256       65.5 KHz          Audio sample rate
3       1024      16.4 KHz          Low-frequency events
```

## Cascading — Chaining Timers Together

When cascade mode (bit 2) is set, the timer doesn't count clock cycles. Instead, it increments only when the **previous timer overflows**.

This lets you chain timers into longer counters:
- TM0 at 16.78 MHz overflows every ~3.9 ms (counting 0→0xFFFF)
- TM1 cascading from TM0 overflows every ~256 seconds
- You now have a 32-bit timer made from two 16-bit timers

Timer 0 can't cascade (there's nothing before it).

### Sound DMA

The most common use of cascading: audio playback.

```
TM0: Runs at audio sample rate (e.g., 16 KHz)
     On overflow → triggers sound DMA to move next sample to FIFO
     Result: one audio sample played per overflow = steady audio stream
```

We haven't implemented sound DMA yet, but the timer is the clock that drives it.

## Enable Edge Behavior

When the enable bit transitions from 0 to 1 (timer just turned on), the hardware:
1. Loads the reload value into the counter
2. Resets the prescaler accumulator

This means writing a reload value and then enabling the timer guarantees the counter starts at exactly that value. If the timer is already running, writing a new reload doesn't affect the current count — it only takes effect on the next overflow.

## Implementation

We track each timer as:
- `counter: u16` — the current count
- `reload: u16` — value loaded on overflow/enable
- `prescaler_counter: u32` — accumulated cycles not yet converted to ticks

In `tick()`, for each non-cascade timer:
1. Add CPU cycles to the prescaler accumulator
2. Divide by the prescaler to get timer ticks
3. Add ticks to the counter
4. On overflow: reload, fire IRQ if enabled, feed cascade timers

The cascade chain is handled recursively — when timer N overflows, we call `timer_add(N+1, overflow_count)` if timer N+1 is in cascade mode.

## What Games Use Timers For

- **Audio**: Timer 0/1 clocks the sound DMA FIFOs for music and sound effects
- **Animation**: Some games use timer interrupts to pace sprite animations independently of the main game loop
- **Profiling**: Developers read timer values to measure how long code sections take
- **Random numbers**: The fast-ticking timer counter serves as a source of randomness (read it when the player presses a button for an unpredictable seed)
- **Timeout/watchdog**: Set a timer to overflow after N milliseconds, use the IRQ as a deadline
