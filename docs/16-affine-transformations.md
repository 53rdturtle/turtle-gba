# Milestone 16: Affine Transformations — Rotation and Scaling

The GBA can rotate and scale both backgrounds and sprites using hardware affine transformations. This is how Wario Land 4 creates its spinning black hole level-entry effect and Wario's shrink/enlarge animations.

## What Is an Affine Transformation?

Imagine you have a background image made of tiles. Normally, screen pixel (100, 50) maps directly to background pixel (100, 50). But what if you want to rotate the whole image 45 degrees, or zoom to 2x?

The trick: **work backwards**. Instead of asking "where does each background pixel land on screen?" (which leaves gaps), ask: "for each screen pixel, where in the background should I sample?"

This reverse mapping is clean — every screen pixel gets exactly one color, no gaps.

## The 2x2 Matrix

The GBA uses four parameters (PA, PB, PC, PD) that form a 2x2 transformation matrix, plus a reference point (X0, Y0):

```
For screen pixel at offset (dx, dy) from the reference point:

  texture_x = X0 + PA * dx + PB * dy
  texture_y = Y0 + PC * dx + PD * dy
```

The parameters are **8.8 fixed-point** — the value 0x0100 means 1.0, 0x0080 means 0.5, 0xFF00 means -1.0.

### Examples

**Identity (no transform):** PA=1, PB=0, PC=0, PD=1
Each screen pixel maps to the same texture pixel. Normal rendering.

**Zoom 2x:** PA=0.5, PB=0, PC=0, PD=0.5
Each screen pixel step covers half a texture pixel, so the image appears twice as large.

**Rotate 90° clockwise:** PA=0, PB=1, PC=-1, PD=0
The x-axis maps to texture y and vice versa, with a sign flip.

**General rotation by angle θ:**
```
PA =  cos(θ)    PB = sin(θ)
PC = -sin(θ)    PD = cos(θ)
```

The game updates these values every frame with increasing angle to create the spinning vortex effect.

## Affine Backgrounds (Mode 1/2)

### How They Differ from Regular Backgrounds

| Feature | Regular (Text) BG | Affine BG |
|---------|-------------------|-----------|
| Tile map entry | 16-bit (tile + flip + palette) | 8-bit (tile index only) |
| Color mode | 4bpp or 8bpp | Always 8bpp |
| Flip support | Per-tile H/V flip | No (matrix handles it) |
| Map shape | 32x32 to 64x64 tiles | 16x16 to 128x128 tiles (always square) |
| Transform | Scroll only | Full rotation + scaling |
| Wrap mode | Always wraps | BGxCNT bit 13 controls wrap vs transparent |

### GBA Registers

BG2 affine parameters (I/O offsets):
```
0x20: BG2PA    0x22: BG2PB     (16-bit, 8.8 fixed-point)
0x24: BG2PC    0x26: BG2PD     (16-bit, 8.8 fixed-point)
0x28: BG2X     (32-bit, 20.8 fixed-point — reference X)
0x2C: BG2Y     (32-bit, 20.8 fixed-point — reference Y)
```

BG3 uses the same layout at offsets 0x30-0x3C.

### Display Modes

- **Mode 0**: 4 regular BGs (no affine)
- **Mode 1**: BG0 + BG1 regular, BG2 affine
- **Mode 2**: BG2 + BG3 both affine

### Rendering Algorithm

For each screen pixel (sx, sy):
```
tex_x = X0 + PA * sx + PB * sy
tex_y = Y0 + PC * sx + PD * sy
```

Convert from fixed-point to integer (>> 8), then look up the tile and pixel at that coordinate. If the coordinate is out of bounds, either wrap or show transparent depending on the overflow bit.

The reference point (X0, Y0) determines the center of rotation. Games typically set this to the center of the background so rotation happens around the middle of the screen.

## Affine Sprites

### Where the Parameters Live

The GBA stores sprite affine parameters inside OAM (Object Attribute Memory) itself, in a clever space-saving trick. Each OAM entry is 8 bytes:

```
Bytes 0-1: Attr0    Bytes 2-3: Attr1
Bytes 4-5: Attr2    Bytes 6-7: Affine parameter
```

Bytes 6-7 of each entry hold one affine parameter. Since there are 128 entries, that's 128 parameter slots — grouped into **32 affine parameter sets** of 4 values:

```
Group 0:  PA = OAM[0].param    PB = OAM[1].param
          PC = OAM[2].param    PD = OAM[3].param

Group 1:  PA = OAM[4].param    PB = OAM[5].param
          PC = OAM[6].param    PD = OAM[7].param
... up to group 31
```

An affine sprite's attr1 bits 9-13 select which group to use.

### OAM Mode Field

Attr0 bits 8-9 control sprite behavior:
- **Mode 0**: Normal sprite (direct pixel mapping, with H/V flip)
- **Mode 1**: Affine sprite (uses matrix, no flip bits)
- **Mode 2**: Hidden (not drawn)
- **Mode 3**: Affine double-size (2x bounding box)

### Rendering: Center-Based Transform

Unlike backgrounds (which transform from screen origin), affine sprites transform relative to their **center**:

```
For each screen pixel at offset (dx, dy) from the bounding box center:

  tex_x = half_width  + (PA * dx + PB * dy) >> 8
  tex_y = half_height + (PC * dx + PD * dy) >> 8

If tex_x and tex_y are within [0, sprite_width) and [0, sprite_height):
  → draw the pixel from that texture position
Otherwise:
  → transparent (outside the rotated sprite)
```

### Double-Size Mode

When a sprite rotates, its corners extend beyond the original bounding box:

```
Normal bounding box:        Rotated 45°:
┌────────┐                     ╱╲
│        │                   ╱    ╲
│        │                  ╱      ╲
│        │                 ╱   ██   ╲  ← corners clipped!
└────────┘                  ╲      ╱
                             ╲    ╱
                              ╲╱
```

Double-size mode (OAM mode 3) makes the bounding box 2x wider and taller. The sprite texture stays the same size, but it has room to draw rotated pixels that would otherwise be clipped.

## Testing

We verified against Wario Land 4:
- **Spinning black hole**: Mode 1 affine BG2 with rotating PA/PB/PC/PD parameters creates the vortex effect when entering a level
- **Wario scaling**: Affine sprites with scaling parameters handle Wario's shrink and enlarge animations
- **Regular gameplay**: Mode 0 tiled backgrounds and normal sprites continue working correctly alongside affine features

## What's Not Implemented Yet

- **BIOS ObjAffineSet (SWI 0x0F)**: A BIOS function that computes PA/PB/PC/PD from angle + scale values and writes them to OAM. Games that use this instead of computing the matrix themselves will need this call.
- **Per-scanline affine updates**: Real hardware can change affine parameters mid-frame (per HBlank) for effects like pseudo-3D floors. Our frame-based renderer uses a single set of parameters for the whole frame.
