# Milestone 14: Sprites (OBJ Layer)

Sprites — called **OBJ** (Objects) on the GBA — are independent graphics that float on top of background layers. Everything that moves independently in a game is typically a sprite: player characters, enemies, projectiles, collectibles, UI elements.

## Why Sprites Are Separate from Backgrounds

Backgrounds tile the entire screen with a repeating grid. They're great for terrain, sky, and static scenery. But a player character needs to:
- Move to any position, not just tile boundaries
- Overlap multiple backgrounds
- Be drawn at any size, not just 8×8 tiles
- Have its own animation independent of the background

Sprites solve all of these: each has its own position, size, palette, and flip settings, controlled independently through OAM (Object Attribute Memory).

## The Three Data Sources

### 1. OAM — Where and How (0x07000000, 1KB)

OAM holds 128 sprite entries, 8 bytes each. Each entry describes one sprite's position, size, appearance, and behavior:

```
Attr0 (2 bytes):
  Bits 0-7:   Y coordinate (0-255, wraps)
  Bits 8-9:   Mode (0=normal, 1=affine, 2=hidden, 3=affine double-size)
  Bits 10-11: GFX mode (0=normal, 1=semi-transparent, 2=OBJ window)
  Bit 13:     Color mode (0=4bpp/16 palettes, 1=8bpp/256 colors)
  Bits 14-15: Shape (0=square, 1=wide, 2=tall)

Attr1 (2 bytes):
  Bits 0-8:   X coordinate (9-bit signed: -256 to +255)
  Bit 12:     Horizontal flip (non-affine only)
  Bit 13:     Vertical flip (non-affine only)
  Bits 14-15: Size (0-3)

Attr2 (2 bytes):
  Bits 0-9:   Base tile index
  Bits 10-11: Priority relative to backgrounds (0=on top)
  Bits 12-15: Palette bank (4bpp mode only)
```

### Shape × Size → Pixel Dimensions

The shape and size fields combine to determine the sprite's dimensions:

```
              Size 0    Size 1    Size 2    Size 3
Square (0):    8×8      16×16     32×32     64×64
Wide (1):     16×8      32×8      32×16     64×32
Tall (2):      8×16      8×32     16×32     32×64
```

This covers every combination a game might need — from tiny 8×8 particles to large 64×64 boss sprites.

### 2. OBJ Tile Data — The Pixels (VRAM 0x06010000)

Sprite tiles live in the second half of VRAM, starting at offset 0x10000 (charblocks 4-5). The format is the same 8×8 tile format used by backgrounds: 4bpp (32 bytes per tile) or 8bpp (64 bytes per tile).

A sprite larger than 8×8 uses multiple tiles. The **tile mapping mode** (DISPCNT bit 6) controls how multi-tile sprites find their tiles:

**1D mapping** (bit 6 = 1): Tiles are laid out linearly in memory. A 16×16 sprite using tile index 10 uses tiles 10, 11, 12, 13 (left-to-right, top-to-bottom). This is simpler and what most games use.

**2D mapping** (bit 6 = 0): Tiles are arranged in a virtual 32-column grid (for 4bpp). A 16×16 sprite at tile 10 uses tiles 10, 11 (top row) and 42, 43 (bottom row, 32 tiles down). This maps directly to VRAM as a 2D array.

### 3. OBJ Palette — The Colors (0x05000200)

Sprites use a separate palette from backgrounds. The OBJ palette occupies the second 256 bytes of Palette RAM (indices 256-511). In 4bpp mode, the palette bank field in Attr2 selects one of 16 sub-palettes of 16 colors each.

## Draw Order and Priority

Sprites are drawn in OAM index order: **OBJ 0 is on top, OBJ 127 is behind**. When two sprites overlap, the lower-numbered one wins. This means the game puts the most important sprites (player, UI) at lower indices.

Each sprite also has a 2-bit priority field that determines its position relative to background layers. Priority 0 sprites draw on top of everything; priority 3 sprites can be behind some backgrounds.

For our initial implementation, we draw all sprites on top of all backgrounds (ignoring BG/OBJ priority interleaving). This is good enough for most scenes.

## Testing Results

We verified sprite rendering against:
- **Wario Land 4**: The black cat from the intro cutscene renders correctly as a sprite over the hallway background
- **Tonc obj_demo**: A metroid sprite renders with correct tile layout, palette, and transparency

## What We Haven't Implemented Yet

- **Affine sprites**: Rotation and scaling using the affine matrix in OAM
- **OBJ/BG priority interleaving**: Sprites should interleave with BG layers by priority, not just draw on top of everything
- **Semi-transparent sprites**: GFX mode 1 makes sprites blend with the layer below
- **OBJ window**: GFX mode 2 uses the sprite shape as a window mask
- **Per-scanline rendering**: Currently we render all sprites for the whole frame at once. Real hardware renders per-scanline with a limit of ~960 sprite pixels per line.
