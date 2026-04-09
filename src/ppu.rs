/// PPU (Pixel Processing Unit) — the GBA's graphics hardware.
///
/// The GBA screen is 240×160 pixels. The PPU composites up to 4 background
/// layers + sprites (OBJ) with per-pixel priority and hardware alpha blending.
///
/// ## GBA display modes (DISPCNT bits 0-2):
///   Mode 0: 4 tiled backgrounds (most common in games)
///   Mode 1: 2 tiled + 1 affine (rotation/scaling) background
///   Mode 2: 2 affine backgrounds
///   Mode 3: 16-bit bitmap (240×160, direct color — no palette)
///   Mode 4: 8-bit bitmap (240×160, 256-color palette, double-buffered)
///   Mode 5: 16-bit bitmap (160×128, smaller but double-buffered)

pub const SCREEN_WIDTH: usize = 240;
pub const SCREEN_HEIGHT: usize = 160;

/// A pixel from a single layer, before compositing.
/// `TRANSPARENT` means this layer has nothing at this position.
const TRANSPARENT: u32 = 0xFF000000;

/// Layer identifiers — used to check BLDCNT first/second target bits.
const LAYER_BG0: u8 = 0;
const LAYER_BG1: u8 = 1;
const LAYER_BG2: u8 = 2;
const LAYER_BG3: u8 = 3;
const LAYER_OBJ: u8 = 4;
const LAYER_BD:  u8 = 5; // Backdrop (palette[0])

/// Render one full frame with proper layer compositing and blending.
pub fn render_frame(vram: &[u8], palette: &[u8], oam: &[u8], io: &[u8]) -> Vec<u32> {
    let dispcnt = (io[0] as u16) | ((io[1] as u16) << 8);
    let mode = dispcnt & 0x7;

    match mode {
        0 => render_composited(vram, palette, oam, io, dispcnt),
        1 | 2 => render_composited(vram, palette, oam, io, dispcnt),
        3 => render_mode3_blended(vram, io),
        4 => render_mode4_blended(vram, palette, oam, io, dispcnt),
        _ => vec![0x00333333; SCREEN_WIDTH * SCREEN_HEIGHT],
    }
}

/// Full compositing pipeline for Mode 0: tiled backgrounds + sprites + blending.
///
/// For each pixel we find the two highest-priority opaque layers, then apply
/// the blend effect selected by BLDCNT.
fn render_composited(
    vram: &[u8], palette: &[u8], oam: &[u8], io: &[u8], dispcnt: u16,
) -> Vec<u32> {
    // Read blend control registers
    let bldcnt = read_io_u16(io, 0x50);
    let blend_mode = (bldcnt >> 6) & 3;
    let eva = (read_io_u16(io, 0x52) & 0x1F).min(16) as u32;
    let evb = ((read_io_u16(io, 0x52) >> 8) & 0x1F).min(16) as u32;
    let evy = (read_io_u16(io, 0x54) & 0x1F).min(16) as u32;

    // Collect enabled BGs sorted by priority (highest priority number drawn first = behind)
    let mut bg_layers: Vec<(u8, usize)> = Vec::new(); // (priority, bg_index)
    for bg in 0..4usize {
        if dispcnt & (1 << (8 + bg)) != 0 {
            let bgcnt = read_io_u16(io, 0x08 + bg * 2);
            let priority = (bgcnt & 0x3) as u8;
            bg_layers.push((priority, bg));
        }
    }

    let obj_enabled = dispcnt & (1 << 12) != 0;

    // Pre-render each BG layer into a pixel buffer (TRANSPARENT where no pixel)
    let mut bg_pixels: [Vec<u32>; 4] = [
        vec![TRANSPARENT; SCREEN_WIDTH * SCREEN_HEIGHT],
        vec![TRANSPARENT; SCREEN_WIDTH * SCREEN_HEIGHT],
        vec![TRANSPARENT; SCREEN_WIDTH * SCREEN_HEIGHT],
        vec![TRANSPARENT; SCREEN_WIDTH * SCREEN_HEIGHT],
    ];
    let mut bg_priorities: [u8; 4] = [3; 4]; // Default to lowest priority

    let mode = dispcnt & 0x7;

    for &(priority, bg) in &bg_layers {
        let bgcnt = read_io_u16(io, 0x08 + bg * 2);
        bg_priorities[bg] = priority;

        // In Mode 1, BG2 is affine. In Mode 2, BG2 and BG3 are affine.
        let is_affine = match mode {
            1 => bg == 2,
            2 => bg == 2 || bg == 3,
            _ => false,
        };

        if is_affine {
            render_affine_bg_layer(vram, palette, io, &mut bg_pixels[bg], bg, bgcnt);
        } else {
            render_text_bg_layer(vram, palette, io, &mut bg_pixels[bg], bg, bgcnt);
        }
    }

    // Pre-render OBJ layer: color + per-pixel priority
    let mut obj_pixels = vec![TRANSPARENT; SCREEN_WIDTH * SCREEN_HEIGHT];
    let mut obj_prio = vec![3u8; SCREEN_WIDTH * SCREEN_HEIGHT]; // per-pixel OBJ priority
    if obj_enabled {
        render_obj_layer(vram, palette, oam, dispcnt, &mut obj_pixels, &mut obj_prio);
    }

    // Backdrop color
    let backdrop = palette_to_rgb(palette, 0);

    // Composite per pixel
    let mut framebuf = vec![0u32; SCREEN_WIDTH * SCREEN_HEIGHT];

    for i in 0..(SCREEN_WIDTH * SCREEN_HEIGHT) {
        // Build a sorted list of (priority, layer_id, color) for opaque layers.
        // Priority order: lower number = on top. Ties broken by: OBJ before BG,
        // lower BG number before higher.
        let mut top_color = backdrop;
        let mut top_layer = LAYER_BD;
        let mut second_color = backdrop;
        let mut second_layer = LAYER_BD;
        let mut found_top = false;

        // We iterate in priority order: 0 (highest) through 3 (lowest).
        // At each priority level, OBJ comes before BGs; BGs in index order.
        for prio in 0..4u8 {
            // Check OBJ at this priority
            if obj_pixels[i] != TRANSPARENT && obj_prio[i] == prio {
                if !found_top {
                    top_color = obj_pixels[i];
                    top_layer = LAYER_OBJ;
                    found_top = true;
                } else {
                    second_color = obj_pixels[i];
                    second_layer = LAYER_OBJ;
                    break;
                }
            }
            // Check BGs at this priority
            for bg in 0..4usize {
                if bg_priorities[bg] == prio && bg_pixels[bg][i] != TRANSPARENT {
                    if !found_top {
                        top_color = bg_pixels[bg][i];
                        top_layer = bg as u8;
                        found_top = true;
                    } else {
                        second_color = bg_pixels[bg][i];
                        second_layer = bg as u8;
                        break;
                    }
                }
            }
            if found_top && second_layer != LAYER_BD {
                break; // Found both layers
            }
        }

        if !found_top {
            framebuf[i] = backdrop;
            continue;
        }

        // Apply blending
        let is_first_target = (bldcnt >> top_layer) & 1 == 1;
        let is_second_target = (bldcnt >> (8 + second_layer)) & 1 == 1;

        framebuf[i] = match blend_mode {
            1 if is_first_target && is_second_target => {
                alpha_blend(top_color, second_color, eva, evb)
            }
            2 if is_first_target => {
                brightness_up(top_color, evy)
            }
            3 if is_first_target => {
                brightness_down(top_color, evy)
            }
            _ => top_color,
        };
    }

    framebuf
}

/// Mode 4 with blending and OBJ support.
fn render_mode4_blended(
    vram: &[u8], palette: &[u8], oam: &[u8], io: &[u8], dispcnt: u16,
) -> Vec<u32> {
    let bldcnt = read_io_u16(io, 0x50);
    let blend_mode = (bldcnt >> 6) & 3;
    let eva = (read_io_u16(io, 0x52) & 0x1F).min(16) as u32;
    let evb = ((read_io_u16(io, 0x52) >> 8) & 0x1F).min(16) as u32;
    let evy = (read_io_u16(io, 0x54) & 0x1F).min(16) as u32;

    let frame_offset = if dispcnt & (1 << 4) != 0 { 0xA000 } else { 0 };
    let obj_enabled = dispcnt & (1 << 12) != 0;

    // Render BG2 (Mode 4 uses BG2)
    let mut bg_pixels = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT);
    for y in 0..SCREEN_HEIGHT {
        for x in 0..SCREEN_WIDTH {
            let idx = frame_offset + y * SCREEN_WIDTH + x;
            let pal_idx = if idx < vram.len() { vram[idx] as usize } else { 0 };
            if pal_idx == 0 {
                bg_pixels.push(TRANSPARENT);
            } else {
                bg_pixels.push(palette_to_rgb(palette, pal_idx));
            }
        }
    }

    // Render OBJ layer
    let mut obj_pixels = vec![TRANSPARENT; SCREEN_WIDTH * SCREEN_HEIGHT];
    let mut obj_prio = vec![3u8; SCREEN_WIDTH * SCREEN_HEIGHT];
    if obj_enabled {
        render_obj_layer(vram, palette, oam, dispcnt, &mut obj_pixels, &mut obj_prio);
    }

    let backdrop = palette_to_rgb(palette, 0);

    let mut framebuf = vec![0u32; SCREEN_WIDTH * SCREEN_HEIGHT];
    for i in 0..(SCREEN_WIDTH * SCREEN_HEIGHT) {
        // Mode 4 BG2 is at priority from BG2CNT
        let bg2cnt = read_io_u16(io, 0x0C);
        let bg_prio = (bg2cnt & 3) as u8;

        // Determine top and second layer
        let (top_color, top_layer, second_color, second_layer);
        let obj_visible = obj_pixels[i] != TRANSPARENT;
        let bg_visible = bg_pixels[i] != TRANSPARENT;

        if obj_visible && (!bg_visible || obj_prio[i] <= bg_prio) {
            top_color = obj_pixels[i];
            top_layer = LAYER_OBJ;
            second_color = if bg_visible { bg_pixels[i] } else { backdrop };
            second_layer = if bg_visible { LAYER_BG2 } else { LAYER_BD };
        } else if bg_visible {
            top_color = bg_pixels[i];
            top_layer = LAYER_BG2;
            second_color = if obj_visible { obj_pixels[i] } else { backdrop };
            second_layer = if obj_visible { LAYER_OBJ } else { LAYER_BD };
        } else {
            framebuf[i] = backdrop;
            continue;
        }

        let is_first_target = (bldcnt >> top_layer) & 1 == 1;
        let is_second_target = (bldcnt >> (8 + second_layer)) & 1 == 1;

        framebuf[i] = match blend_mode {
            1 if is_first_target && is_second_target => {
                alpha_blend(top_color, second_color, eva, evb)
            }
            2 if is_first_target => brightness_up(top_color, evy),
            3 if is_first_target => brightness_down(top_color, evy),
            _ => top_color,
        };
    }

    framebuf
}

/// Mode 3 with brightness blending (no OBJ for now).
fn render_mode3_blended(vram: &[u8], io: &[u8]) -> Vec<u32> {
    let bldcnt = read_io_u16(io, 0x50);
    let blend_mode = (bldcnt >> 6) & 3;
    let evy = (read_io_u16(io, 0x54) & 0x1F).min(16) as u32;

    let mut framebuf = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT);
    for y in 0..SCREEN_HEIGHT {
        for x in 0..SCREEN_WIDTH {
            let offset = (y * SCREEN_WIDTH + x) * 2;
            let color = if offset + 1 < vram.len() {
                let bgr555 = (vram[offset] as u16) | ((vram[offset + 1] as u16) << 8);
                bgr555_to_rgb(bgr555)
            } else {
                0
            };

            let is_first_target = (bldcnt >> LAYER_BG2) & 1 == 1;
            let blended = match blend_mode {
                2 if is_first_target => brightness_up(color, evy),
                3 if is_first_target => brightness_down(color, evy),
                _ => color,
            };
            framebuf.push(blended);
        }
    }
    framebuf
}

// --- Layer rendering (no compositing, just raw pixel data) ---

/// Render a tiled background into a pixel buffer. Transparent pixels = TRANSPARENT.
fn render_text_bg_layer(
    vram: &[u8], palette: &[u8], io: &[u8],
    pixels: &mut [u32], bg: usize, bgcnt: u16,
) {
    let cbb = ((bgcnt >> 2) & 0x3) as usize;
    let sbb = ((bgcnt >> 8) & 0x1F) as usize;
    let is_8bpp = bgcnt & (1 << 7) != 0;
    let screen_size = (bgcnt >> 14) & 0x3;

    let hofs = read_io_u16(io, 0x10 + bg * 4) & 0x1FF;
    let vofs = read_io_u16(io, 0x12 + bg * 4) & 0x1FF;

    let (bg_width_tiles, bg_height_tiles) = match screen_size {
        0 => (32, 32),
        1 => (64, 32),
        2 => (32, 64),
        3 => (64, 64),
        _ => unreachable!(),
    };

    let tile_base = cbb * 0x4000;
    let map_base = sbb * 0x800;

    for screen_y in 0..SCREEN_HEIGHT {
        for screen_x in 0..SCREEN_WIDTH {
            let bg_x = (screen_x as u16 + hofs) & (bg_width_tiles as u16 * 8 - 1);
            let bg_y = (screen_y as u16 + vofs) & (bg_height_tiles as u16 * 8 - 1);

            let tile_x = (bg_x / 8) as usize;
            let tile_y = (bg_y / 8) as usize;
            let pixel_x = (bg_x % 8) as usize;
            let pixel_y = (bg_y % 8) as usize;

            let sbb_x = tile_x / 32;
            let sbb_y = tile_y / 32;
            let sbb_offset = match screen_size {
                0 => 0,
                1 => sbb_x,
                2 => sbb_y,
                3 => sbb_x + sbb_y * 2,
                _ => 0,
            };
            let local_x = tile_x % 32;
            let local_y = tile_y % 32;

            let se_addr = map_base + sbb_offset * 0x800 + (local_y * 32 + local_x) * 2;
            let se = if se_addr + 1 < vram.len() {
                (vram[se_addr] as u16) | ((vram[se_addr + 1] as u16) << 8)
            } else {
                0
            };

            let tile_id = (se & 0x3FF) as usize;
            let h_flip = se & (1 << 10) != 0;
            let v_flip = se & (1 << 11) != 0;
            let pal_bank = ((se >> 12) & 0xF) as usize;

            let px = if h_flip { 7 - pixel_x } else { pixel_x };
            let py = if v_flip { 7 - pixel_y } else { pixel_y };

            let color_index = if is_8bpp {
                let addr = tile_base + tile_id * 64 + py * 8 + px;
                if addr < vram.len() { vram[addr] as usize } else { 0 }
            } else {
                let addr = tile_base + tile_id * 32 + py * 4 + px / 2;
                let byte = if addr < vram.len() { vram[addr] } else { 0 };
                if px & 1 == 0 { (byte & 0x0F) as usize } else { ((byte >> 4) & 0x0F) as usize }
            };

            if color_index == 0 {
                continue; // Transparent — leave as TRANSPARENT
            }

            let pal_index = if is_8bpp { color_index } else { pal_bank * 16 + color_index };
            pixels[screen_y * SCREEN_WIDTH + screen_x] = palette_to_rgb(palette, pal_index);
        }
    }
}

/// Render an affine (rotation/scaling) background layer.
///
/// Affine BGs use a 2x2 matrix (PA, PB, PC, PD) and a reference point (X0, Y0)
/// to transform screen coordinates into background texture coordinates:
///
///   tex_x = X0 + PA * screen_x + PB * screen_y
///   tex_y = Y0 + PC * screen_x + PD * screen_y
///
/// Key differences from regular (text) backgrounds:
///   - Tile map entries are 8 bits (just a tile index, no flip/palette)
///   - Always 8bpp (256-color, single palette)
///   - Map is a square: 16x16, 32x32, 64x64, or 128x128 tiles
///   - Overflow bit (BGxCNT bit 13) controls wrapping vs transparent
fn render_affine_bg_layer(
    vram: &[u8], palette: &[u8], io: &[u8],
    pixels: &mut [u32], bg: usize, bgcnt: u16,
) {
    let cbb = ((bgcnt >> 2) & 0x3) as usize;
    let sbb = ((bgcnt >> 8) & 0x1F) as usize;
    let wraparound = bgcnt & (1 << 13) != 0;
    let size_bits = (bgcnt >> 14) & 0x3;

    // Affine BG sizes (in tiles): 16x16, 32x32, 64x64, 128x128
    let map_size = match size_bits {
        0 => 16,
        1 => 32,
        2 => 64,
        3 => 128,
        _ => unreachable!(),
    };
    let bg_size_px = map_size * 8; // Size in pixels

    let tile_base = cbb * 0x4000;
    let map_base = sbb * 0x800;

    // Read affine parameters from I/O registers.
    // BG2: PA=0x20, PB=0x22, PC=0x24, PD=0x26, X=0x28, Y=0x2C
    // BG3: PA=0x30, PB=0x32, PC=0x34, PD=0x36, X=0x38, Y=0x3C
    let param_base = if bg == 3 { 0x30 } else { 0x20 };

    // PA/PB/PC/PD are 16-bit signed, 8.8 fixed-point
    let pa = read_io_i16(io, param_base) as i32;
    let pb = read_io_i16(io, param_base + 2) as i32;
    let pc = read_io_i16(io, param_base + 4) as i32;
    let pd = read_io_i16(io, param_base + 6) as i32;

    // Reference point X0/Y0 are 32-bit signed, 20.8 fixed-point
    // (28 bits used: 20 integer + 8 fractional, sign-extended to 32)
    let ref_base = if bg == 3 { 0x38 } else { 0x28 };
    let x0 = read_io_i32(io, ref_base);
    let y0 = read_io_i32(io, ref_base + 4);

    for screen_y in 0..SCREEN_HEIGHT {
        // Texture coordinate at (0, screen_y), then we walk right adding (PA, PC)
        let mut tex_x = x0 + pb * screen_y as i32;
        let mut tex_y = y0 + pd * screen_y as i32;

        for screen_x in 0..SCREEN_WIDTH {
            // Convert from 8.8 fixed-point to integer pixel coordinates
            let bg_x = tex_x >> 8;
            let bg_y = tex_y >> 8;

            // Advance texture position for next pixel
            tex_x += pa;
            tex_y += pc;

            // Check bounds
            let (final_x, final_y) = if wraparound {
                // Wrap around the background
                (bg_x.rem_euclid(bg_size_px as i32), bg_y.rem_euclid(bg_size_px as i32))
            } else {
                // Out of bounds = transparent
                if bg_x < 0 || bg_y < 0 || bg_x >= bg_size_px as i32 || bg_y >= bg_size_px as i32 {
                    continue;
                }
                (bg_x, bg_y)
            };

            // Which tile in the map?
            let tile_x = (final_x / 8) as usize;
            let tile_y = (final_y / 8) as usize;
            let pixel_in_tile_x = (final_x % 8) as usize;
            let pixel_in_tile_y = (final_y % 8) as usize;

            // Affine map entries are 8 bits (just a tile index)
            let map_addr = map_base + tile_y * map_size + tile_x;
            let tile_id = if map_addr < vram.len() { vram[map_addr] as usize } else { 0 };

            // Always 8bpp: 64 bytes per tile, 1 byte per pixel
            let pixel_addr = tile_base + tile_id * 64 + pixel_in_tile_y * 8 + pixel_in_tile_x;
            let color_index = if pixel_addr < vram.len() { vram[pixel_addr] as usize } else { 0 };

            if color_index == 0 { continue; } // Transparent

            pixels[screen_y * SCREEN_WIDTH + screen_x] = palette_to_rgb(palette, color_index);
        }
    }
}

/// Read a 16-bit signed value from IO registers.
fn read_io_i16(io: &[u8], offset: usize) -> i16 {
    ((io[offset] as u16) | ((io[offset + 1] as u16) << 8)) as i16
}

/// Read a 32-bit signed value from IO registers (for affine reference points).
/// The value is 20.8 fixed-point, sign-extended from 28 bits.
fn read_io_i32(io: &[u8], offset: usize) -> i32 {
    let raw = (io[offset] as u32)
        | ((io[offset + 1] as u32) << 8)
        | ((io[offset + 2] as u32) << 16)
        | ((io[offset + 3] as u32) << 24);
    // Sign-extend from bit 27
    if raw & (1 << 27) != 0 {
        (raw | 0xF000_0000) as i32
    } else {
        (raw & 0x0FFF_FFFF) as i32
    }
}

/// Look up a pixel's color index from OBJ tile data.
///
/// Given texture coordinates within a sprite, finds the correct tile
/// and reads the palette index. Returns 0 for transparent.
fn obj_tile_pixel(
    vram: &[u8], tex_x: usize, tex_y: usize,
    obj_w: usize, tile_id: usize, is_8bpp: bool, mapping_1d: bool,
) -> usize {
    let tile_base: usize = 0x10000;
    let tile_col = tex_x / 8;
    let tile_row = tex_y / 8;
    let pixel_in_tile_x = tex_x % 8;
    let pixel_in_tile_y = tex_y % 8;

    let actual_tile = if mapping_1d {
        if is_8bpp {
            tile_id + tile_row * (obj_w / 8) * 2 + tile_col * 2
        } else {
            tile_id + tile_row * (obj_w / 8) + tile_col
        }
    } else {
        let tiles_per_row = if is_8bpp { 16 } else { 32 };
        tile_id + tile_row * tiles_per_row + tile_col
    };

    if is_8bpp {
        let addr = tile_base + actual_tile * 32 + pixel_in_tile_y * 8 + pixel_in_tile_x;
        if addr < vram.len() { vram[addr] as usize } else { 0 }
    } else {
        let addr = tile_base + actual_tile * 32 + pixel_in_tile_y * 4 + pixel_in_tile_x / 2;
        let byte = if addr < vram.len() { vram[addr] } else { 0 };
        if pixel_in_tile_x & 1 == 0 {
            (byte & 0x0F) as usize
        } else {
            ((byte >> 4) & 0x0F) as usize
        }
    }
}

/// Read an OBJ affine parameter group from OAM.
///
/// Affine parameters are stored in bytes 6-7 of every OAM entry,
/// grouped in sets of 4 entries. Group N uses entries N*4..N*4+3.
fn read_obj_affine(oam: &[u8], group: usize) -> (i16, i16, i16, i16) {
    let pa_off = group * 32 + 6;   // Entry N*4+0, bytes 6-7
    let pb_off = group * 32 + 14;  // Entry N*4+1, bytes 6-7
    let pc_off = group * 32 + 22;  // Entry N*4+2, bytes 6-7
    let pd_off = group * 32 + 30;  // Entry N*4+3, bytes 6-7
    let read = |off: usize| -> i16 {
        if off + 1 < oam.len() {
            ((oam[off] as u16) | ((oam[off + 1] as u16) << 8)) as i16
        } else { 0x100 } // 1.0 in 8.8 fixed-point as fallback
    };
    (read(pa_off), read(pb_off), read(pc_off), read(pd_off))
}

/// Render all OBJ sprites (regular and affine) into pixel and priority buffers.
fn render_obj_layer(
    vram: &[u8], palette: &[u8], oam: &[u8], dispcnt: u16,
    pixels: &mut [u32], priorities: &mut [u8],
) {
    let mapping_1d = dispcnt & (1 << 6) != 0;

    let obj_sizes: [[(usize, usize); 4]; 3] = [
        [(8,8), (16,16), (32,32), (64,64)],
        [(16,8), (32,8), (32,16), (64,32)],
        [(8,16), (8,32), (16,32), (32,64)],
    ];

    // Draw in reverse order so lower-numbered OBJs overwrite higher-numbered ones
    for obj_num in (0..128).rev() {
        let base = obj_num * 8;
        if base + 5 >= oam.len() { continue; }

        let attr0 = (oam[base] as u16) | ((oam[base + 1] as u16) << 8);
        let attr1 = (oam[base + 2] as u16) | ((oam[base + 3] as u16) << 8);
        let attr2 = (oam[base + 4] as u16) | ((oam[base + 5] as u16) << 8);

        let obj_mode = (attr0 >> 8) & 0x3;
        if obj_mode == 2 { continue; }  // Hidden

        let is_affine = obj_mode == 1 || obj_mode == 3;
        let is_double_size = obj_mode == 3;

        let is_8bpp = attr0 & (1 << 13) != 0;
        let shape = ((attr0 >> 14) & 0x3) as usize;
        let size = ((attr1 >> 14) & 0x3) as usize;
        if shape > 2 { continue; }

        let (obj_w, obj_h) = obj_sizes[shape][size];
        let obj_priority = ((attr2 >> 10) & 0x3) as u8;

        let y = (attr0 & 0xFF) as i32;
        let x = {
            let raw = (attr1 & 0x1FF) as i32;
            if raw >= 256 { raw - 512 } else { raw }
        };

        let tile_id = (attr2 & 0x3FF) as usize;
        let pal_bank = ((attr2 >> 12) & 0xF) as usize;

        // Bounding box on screen: double-size sprites use 2x the area
        let (bound_w, bound_h) = if is_double_size {
            (obj_w * 2, obj_h * 2)
        } else {
            (obj_w, obj_h)
        };

        if is_affine {
            // Read affine parameters from OAM
            let affine_group = ((attr1 >> 9) & 0x1F) as usize;
            let (pa, pb, pc, pd) = read_obj_affine(oam, affine_group);
            let pa = pa as i32;
            let pb = pb as i32;
            let pc = pc as i32;
            let pd = pd as i32;

            // The center of the sprite in texture space
            let half_w = (obj_w / 2) as i32;
            let half_h = (obj_h / 2) as i32;
            // The center of the bounding box on screen
            let center_x = (bound_w / 2) as i32;
            let center_y = (bound_h / 2) as i32;

            for py in 0..bound_h {
                let screen_y = (y + py as i32) & 0xFF;
                if screen_y < 0 || screen_y >= SCREEN_HEIGHT as i32 { continue; }

                // Offset from bounding box center
                let dy = py as i32 - center_y;

                for px in 0..bound_w {
                    let screen_x = x + px as i32;
                    if screen_x < 0 || screen_x >= SCREEN_WIDTH as i32 { continue; }

                    let dx = px as i32 - center_x;

                    // Apply affine matrix to find texture coordinate
                    let tex_x = half_w + ((pa * dx + pb * dy) >> 8);
                    let tex_y = half_h + ((pc * dx + pd * dy) >> 8);

                    // Check if inside the sprite texture
                    if tex_x < 0 || tex_y < 0 || tex_x >= obj_w as i32 || tex_y >= obj_h as i32 {
                        continue;
                    }

                    let color_index = obj_tile_pixel(
                        vram, tex_x as usize, tex_y as usize,
                        obj_w, tile_id, is_8bpp, mapping_1d,
                    );
                    if color_index == 0 { continue; }

                    let pal_index = if is_8bpp {
                        256 + color_index
                    } else {
                        256 + pal_bank * 16 + color_index
                    };

                    let si = screen_y as usize * SCREEN_WIDTH + screen_x as usize;
                    pixels[si] = palette_to_rgb(palette, pal_index);
                    priorities[si] = obj_priority;
                }
            }
        } else {
            // Regular (non-affine) sprite — direct pixel mapping with optional flip
            let h_flip = attr1 & (1 << 12) != 0;
            let v_flip = attr1 & (1 << 13) != 0;

            for py in 0..obj_h {
                let screen_y = (y + py as i32) & 0xFF;
                if screen_y < 0 || screen_y >= SCREEN_HEIGHT as i32 { continue; }

                for px in 0..obj_w {
                    let screen_x = x + px as i32;
                    if screen_x < 0 || screen_x >= SCREEN_WIDTH as i32 { continue; }

                    let tex_x = if h_flip { obj_w - 1 - px } else { px };
                    let tex_y = if v_flip { obj_h - 1 - py } else { py };

                    let color_index = obj_tile_pixel(
                        vram, tex_x, tex_y,
                        obj_w, tile_id, is_8bpp, mapping_1d,
                    );
                    if color_index == 0 { continue; }

                    let pal_index = if is_8bpp {
                        256 + color_index
                    } else {
                        256 + pal_bank * 16 + color_index
                    };

                    let si = screen_y as usize * SCREEN_WIDTH + screen_x as usize;
                    pixels[si] = palette_to_rgb(palette, pal_index);
                    priorities[si] = obj_priority;
                }
            }
        }
    }
}

// --- Blending ---

/// Alpha blend: result = (color_a * eva + color_b * evb) / 16, clamped to 255.
fn alpha_blend(a: u32, b: u32, eva: u32, evb: u32) -> u32 {
    let r = (((a >> 16) & 0xFF) * eva + ((b >> 16) & 0xFF) * evb) / 16;
    let g = (((a >> 8) & 0xFF) * eva + ((b >> 8) & 0xFF) * evb) / 16;
    let bl = ((a & 0xFF) * eva + (b & 0xFF) * evb) / 16;
    (r.min(255) << 16) | (g.min(255) << 8) | bl.min(255)
}

/// Brightness increase: each component moves toward 255 by evy/16.
fn brightness_up(color: u32, evy: u32) -> u32 {
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    let r2 = r + (255 - r) * evy / 16;
    let g2 = g + (255 - g) * evy / 16;
    let b2 = b + (255 - b) * evy / 16;
    (r2 << 16) | (g2 << 8) | b2
}

/// Brightness decrease: each component moves toward 0 by evy/16.
fn brightness_down(color: u32, evy: u32) -> u32 {
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    let r2 = r - r * evy / 16;
    let g2 = g - g * evy / 16;
    let b2 = b - b * evy / 16;
    (r2 << 16) | (g2 << 8) | b2
}

// --- Utilities ---

fn read_io_u16(io: &[u8], offset: usize) -> u16 {
    (io[offset] as u16) | ((io[offset + 1] as u16) << 8)
}

fn palette_to_rgb(palette: &[u8], index: usize) -> u32 {
    let offset = index * 2;
    if offset + 1 >= palette.len() {
        return 0;
    }
    let bgr555 = (palette[offset] as u16) | ((palette[offset + 1] as u16) << 8);
    bgr555_to_rgb(bgr555)
}

fn bgr555_to_rgb(bgr555: u16) -> u32 {
    let r5 = (bgr555 & 0x1F) as u32;
    let g5 = ((bgr555 >> 5) & 0x1F) as u32;
    let b5 = ((bgr555 >> 10) & 0x1F) as u32;
    let r8 = (r5 << 3) | (r5 >> 2);
    let g8 = (g5 << 3) | (g5 >> 2);
    let b8 = (b5 << 3) | (b5 >> 2);
    (r8 << 16) | (g8 << 8) | b8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgr555_white() {
        assert_eq!(bgr555_to_rgb(0x7FFF), 0x00FFFFFF);
    }

    #[test]
    fn bgr555_black() {
        assert_eq!(bgr555_to_rgb(0x0000), 0x00000000);
    }

    #[test]
    fn bgr555_pure_red() {
        assert_eq!(bgr555_to_rgb(0x001F), 0x00FF0000);
    }

    #[test]
    fn bgr555_pure_green() {
        assert_eq!(bgr555_to_rgb(0x03E0), 0x0000FF00);
    }

    #[test]
    fn bgr555_pure_blue() {
        assert_eq!(bgr555_to_rgb(0x7C00), 0x000000FF);
    }

    #[test]
    fn alpha_blend_equal() {
        // 50/50 blend of red and blue
        let result = alpha_blend(0x00FF0000, 0x000000FF, 8, 8);
        assert_eq!(result, 0x007F007F);
    }

    #[test]
    fn brightness_up_full() {
        // Full brightness up = white
        assert_eq!(brightness_up(0x00000000, 16), 0x00FFFFFF);
    }

    #[test]
    fn brightness_down_full() {
        // Full brightness down = black
        assert_eq!(brightness_down(0x00FFFFFF, 16), 0x00000000);
    }
}
