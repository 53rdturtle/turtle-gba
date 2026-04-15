# Milestone 18: Sound — Direct Sound and Audio Output

The GBA has two audio systems: the legacy PSG (Programmable Sound Generator) inherited from the Game Boy, and **Direct Sound** — two DMA-driven channels that play back arbitrary PCM samples. Almost every GBA game uses Direct Sound for music and sound effects, so that's what we implemented.

## How GBA Audio Works

### The Signal Chain

```
ROM (sample data) → DMA → FIFO → Timer overflow → DAC → Speaker
```

1. **Sample data** lives somewhere in memory (usually ROM or IWRAM)
2. **DMA** copies 16 bytes (4 words) at a time into a 32-byte FIFO queue
3. A **timer overflow** pops one sample from the FIFO and sends it to the output
4. When the FIFO runs low (≤16 samples), DMA automatically refills it

### FIFO — First In, First Out

The GBA has two FIFOs: **FIFO A** and **FIFO B**. Each is a 32-byte queue holding signed 8-bit samples (i8, range -128 to +127). Games typically use one FIFO per audio track — for example, music on FIFO A and sound effects on FIFO B, or left channel on A and right channel on B.

Samples enter the FIFO via:
- **CPU writes** to 0x040000A0 (FIFO A) or 0x040000A4 (FIFO B) — 4 samples per 32-bit write
- **DMA transfers** — same addresses, same 4-samples-per-word packing

### Timer-Driven Playback

The playback rate is controlled by a hardware timer (Timer 0 or Timer 1, selected per-FIFO). Each time the timer overflows, one sample is popped from the FIFO and sent to the Digital-to-Analog Converter (DAC).

For example, Wario Land 4 uses Timer 0 with reload value 0xFB1A and prescaler 1:1:
```
Sample rate = 16,780,000 / (0x10000 - 0xFB1A)
            = 16,780,000 / 1254
            ≈ 13,381 Hz
```

### Sound DMA

DMA channels 1 and 2 can be configured with `start_timing = 3` ("special"), which means they're triggered by FIFO requests rather than VBlank/HBlank. When the FIFO drops below half-full:
1. DMA reads 4 words (16 bytes = 16 samples) from the source address
2. Pushes them into the FIFO
3. Advances the source pointer by 16 bytes
4. The destination stays fixed (always the FIFO address)

The DMA channel stays enabled with `repeat = true`, so it keeps refilling the FIFO indefinitely. The game's audio mixer writes decoded samples to a RAM buffer, and DMA streams them to the FIFO automatically.

### SOUNDCNT_H — The Mixing Register

The register at 0x04000082 controls how the two FIFOs are mixed:

```
Bits 2-3:   Volume (per FIFO, 0=50%, 1=100%)
Bits 8-9:   FIFO A output enable (right, left)
Bit  10:    FIFO A timer select (0=Timer0, 1=Timer1)
Bits 12-13: FIFO B output enable (right, left)
Bit  14:    FIFO B timer select
```

The left/right enable bits determine which speaker each FIFO goes to. Wario Land 4 uses FIFO A → right, FIFO B → left, both driven by Timer 0.

## The Bugs We Hit

### Bug 1: Interleaved Garbage (2x Speed, No Music)

**Symptom**: Audio buffer was being filled at 26,432 samples/sec (2x expected), and the output was garbled noise.

**Root cause**: Both FIFOs used the same timer (Timer 0). Our code processed them independently — each timer overflow pushed one sample from FIFO A, then one from FIFO B, into a single flat buffer. The audio thread treated this as a mono stream: `[A0, B0, A1, B1, A2, B2, ...]`

This meant:
- The audio played at 2x speed (consuming twice as many samples per second)
- Left and right channel data was interleaved into a single stream → garbled

**Fix**: On each timer overflow, pop from both FIFOs simultaneously and mix them into a proper stereo pair: `[left, right]`. Each FIFO contributes to left/right based on its SOUNDCNT_H enable bits.

### Bug 2: Crackling (Buffer Underruns)

**Symptom**: Music was recognizable but full of pops and crackles.

**Root cause**: The audio thread consumed samples based on the *theoretical* GBA clock rate (16.78 MHz → 13,381 Hz sample rate). But the emulator ran at ~59 fps, not 59.73 fps, so it actually produced 13,216 samples/sec. The 1.2% mismatch meant the audio thread occasionally ran out of samples, causing discontinuities.

**Fix**: Adaptive resampling. Instead of a fixed consumption rate, the audio thread adjusts its speed based on buffer fill level:
- **Buffer below target**: slow down (consume fewer samples per output frame)
- **Buffer above target**: speed up (consume more samples)
- **Buffer nearly empty**: output silence instead of repeating stale samples

This automatically compensates for any mismatch between emulator speed and audio consumption rate.

## Debugging Approach

This was a data-driven debugging session. We added counters to track:
1. **Timer overflow rate** — confirmed Timer 0 was overflowing at the expected rate
2. **Samples pushed per second** — revealed the 2x problem (26k vs expected 13k)
3. **Per-FIFO overflow counts** — showed both FIFOs were firing from Timer 0
4. **Buffer fill level** — showed `buf=0` (always empty → underruns)

Each counter narrowed down the problem. The overflow rate being 2x immediately suggested either double-counting or two sources, and the per-FIFO split confirmed both FIFOs shared Timer 0.

## Key Concepts

- **PCM (Pulse Code Modulation)**: Raw digital audio — each sample is a number representing the waveform amplitude at that instant
- **Sample rate**: How many samples per second. Higher = better quality but more data. GBA typically uses 10-32 kHz (CD quality is 44.1 kHz)
- **Ring buffer**: A fixed-size buffer where the producer (emulator) and consumer (audio thread) chase each other. Prevents unbounded memory growth
- **Adaptive resampling**: Dynamically adjusting playback speed to match the producer's rate, rather than assuming a fixed ratio
- **Stereo interleaving**: Audio data stored as alternating left/right samples: `[L0, R0, L1, R1, ...]`
