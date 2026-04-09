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

    for &(priority, bg) in &bg_layers {
        let bgcnt = read_io_u16(io, 0x08 + bg * 2);
        bg_priorities[bg] = priority;
        render_text_bg_layer(vram, palette, io, &mut bg_pixels[bg], bg, bgcnt);
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

/// Render all OBJ sprites into pixel and priority buffers.
fn render_obj_layer(
    vram: &[u8], palette: &[u8], oam: &[u8], dispcnt: u16,
    pixels: &mut [u32], priorities: &mut [u8],
) {
    let tile_base: usize = 0x10000;
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
        if obj_mode == 1 || obj_mode == 3 { continue; } // Affine — skip for now

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

        let h_flip = attr1 & (1 << 12) != 0;
        let v_flip = attr1 & (1 << 13) != 0;
        let tile_id = (attr2 & 0x3FF) as usize;
        let pal_bank = ((attr2 >> 12) & 0xF) as usize;

        for py in 0..obj_h {
            let screen_y = (y + py as i32) & 0xFF;
            if screen_y < 0 || screen_y >= SCREEN_HEIGHT as i32 { continue; }

            for px in 0..obj_w {
                let screen_x = x + px as i32;
                if screen_x < 0 || screen_x >= SCREEN_WIDTH as i32 { continue; }

                let tex_x = if h_flip { obj_w - 1 - px } else { px };
                let tex_y = if v_flip { obj_h - 1 - py } else { py };

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

                let color_index = if is_8bpp {
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
                };

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
