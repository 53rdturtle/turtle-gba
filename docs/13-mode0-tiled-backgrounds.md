# Milestone 13: Mode 0 — Tiled Backgrounds

This is the most important rendering mode on the GBA. While bitmap modes (Mode 3/4/5) draw pixels directly, nearly every commercial game uses **tiled backgrounds** because they're far more memory-efficient and the hardware accelerates scrolling for free.

## Why Tiles?

The GBA screen is 240×160 pixels. In Mode 3 (bitmap), that's 240×160×2 = 76,800 bytes of VRAM just for one frame. VRAM is only 96KB total, so a single bitmap nearly fills it.

With tiles, you define a small set of 8×8 pixel **tiles** (the building blocks) and then a **tile map** that says which tile goes in each position on screen. If many parts of the screen reuse the same tile — as they do in most game backgrounds — you save enormous amounts of memory.

For example, a 256×256 pixel background needs:
- **Tile map**: 32×32 entries × 2 bytes = 2,048 bytes
- **Tile data**: say 128 unique tiles × 32 bytes (4bpp) = 4,096 bytes
- **Total**: ~6KB vs 76KB for a bitmap

Games can have up to 4 tiled backgrounds scrolling independently, with transparency between layers — all driven by hardware, no CPU work per pixel.

## The Three Pieces

### 1. Tile Pixel Data — Charblocks (CBB)

VRAM is divided into 4 **Character Base Blocks** (CBB), each 16KB:

```
CBB 0: 0x06000000 - 0x06003FFF
CBB 1: 0x06004000 - 0x06007FFF
CBB 2: 0x06008000 - 0x0600BFFF
CBB 3: 0x0600C000 - 0x0600FFFF
```

Each tile is 8×8 pixels. In **4 bits-per-pixel (4bpp)** mode, each pixel is a 4-bit palette index, so one tile = 32 bytes. In **8bpp** mode, each pixel is a full byte = 64 bytes per tile.

**4bpp pixel layout** (one row of 8 pixels = 4 bytes):
```
Byte 0: [pixel1:pixel0]   ← low nibble = left pixel
Byte 1: [pixel3:pixel2]
Byte 2: [pixel5:pixel4]
Byte 3: [pixel7:pixel6]
```

The low nibble of each byte is the left pixel, high nibble is the right pixel. This tripped us up initially — it's the opposite of what you might expect.

### 2. Tile Maps — Screenblocks (SBB)

The same VRAM space can also be addressed as 32 **Screen Base Blocks** (SBB), each 2KB:

```
SBB  0: 0x06000000 - 0x060007FF
SBB  1: 0x06000800 - 0x06000FFF
...
SBB 31: 0x0600F800 - 0x0600FFFF
```

Note: CBBs and SBBs overlap in VRAM! CBB 0 contains SBBs 0-7, CBB 1 contains SBBs 8-15, etc. The game must arrange them so tile data and map data don't collide.

Each SBB holds a 32×32 grid of **screen entries** (SE) — one 16-bit entry per tile position:

```
Bits 0-9:   Tile index (0-1023) — which tile from the charblock
Bit 10:     Horizontal flip
Bit 11:     Vertical flip
Bits 12-15: Palette bank (for 4bpp mode — selects which 16-color sub-palette)
```

### 3. Background Control Registers — BGxCNT

Each of the 4 backgrounds (BG0-BG3) has a 16-bit control register:

```
Address     Register
0x04000008  BG0CNT
0x0400000A  BG1CNT
0x0400000C  BG2CNT
0x0400000E  BG3CNT
```

Layout:
```
Bits 0-1:   Priority (0 = highest, drawn on top)
Bits 2-3:   Character Base Block (0-3)
Bit 7:      Color mode (0 = 4bpp with 16 palettes, 1 = 8bpp with 1 palette)
Bits 8-12:  Screen Base Block (0-31)
Bits 14-15: Screen size
```

Screen sizes:
```
Value  Size       SBB layout
0      256×256    [0]
1      512×256    [0][1]       (side by side)
2      256×512    [0]          (stacked)
                  [1]
3      512×512    [0][1]       (2×2 grid)
                  [2][3]
```

## How Rendering Works

For each pixel on screen:

1. **Add scroll offset**: Each BG has horizontal/vertical scroll registers (BGxHOFS/VOFS). The screen-space coordinate (0,0)-(239,159) is offset by the scroll values to get the background-space coordinate.

2. **Find the tile**: Divide the background coordinate by 8 to get the tile position. For maps larger than 32×32, figure out which screenblock the tile is in (the multi-SBB layout above).

3. **Read the screen entry**: Index into the screenblock to get the 16-bit entry containing the tile index, flip flags, and palette bank.

4. **Read the pixel**: Use the tile index to find the pixel data in the charblock. Apply horizontal/vertical flip to the pixel coordinates within the tile. Read the 4-bit (or 8-bit) color index.

5. **Apply palette**: For 4bpp, combine the palette bank (from the screen entry) with the color index to get the final palette entry. For 8bpp, the color index is used directly.

6. **Transparency**: Color index 0 is transparent — the pixel below shows through. This is how multiple background layers combine.

## Priority and Layer Compositing

When multiple backgrounds are enabled, they're drawn in priority order. Lower priority numbers are drawn on top:

```rust
// Sort by priority (highest number = drawn first = behind)
bgs.sort_by(|a, b| b.priority.cmp(&a.priority).then(b.index.cmp(&a.index)));

for bg in &bgs {
    render_text_bg(..., bg);  // Each BG overwrites non-transparent pixels
}
```

When two BGs have the same priority value, the lower-numbered BG is drawn on top (BG0 beats BG1).

## Testing with Tonc Demos

We verified our implementation against two Tonc demos:

- **cbb_demo.gba**: Shows tiles from different charblocks with colored text labels ("01", "02", etc.) in different palette banks. Confirmed that CBB indexing, 4bpp pixel decoding, and palette bank selection all work.

- **sbb_reg.gba**: Shows a scrollable 512×512 tile map made of a red grid pattern. Confirmed that screenblock layout, tile map reading, and basic rendering all work.

## What We Haven't Implemented Yet

- **Affine backgrounds** (Mode 1/2): Rotation and scaling of background layers
- **Sprites / OBJ layer**: Game characters drawn on top of backgrounds
- **Window masking**: Rectangular regions that clip background layers
- **Mosaic effect**: Pixelation effect controlled by hardware
- **Alpha blending**: Semi-transparent layers
- **Per-scanline rendering**: Currently we render the whole frame at once. Real hardware renders scanline-by-scanline, which matters for mid-frame scroll changes (used for parallax effects).

## Key Takeaway

The tile system is a **lookup table of lookup tables**: the screen entry points to a tile, the tile data points to a palette index, and the palette maps to a color. Understanding this chain of indirections is the key to understanding how GBA graphics work. Every layer adds flexibility — games can change the palette to recolor everything, swap a tile to animate, or change a map entry to scroll the world — all without touching pixel data.
