use base64::Engine;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui_image::picker::{Picker, ProtocolType};
use std::io::{self, IsTerminal, Write};

const SPRITE_BASE_ID: u32 = 99_901;
const PLACEMENT_ID: u32 = 99_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    CellCross,
    FillCrosshair,
    ResizeEW,
    ResizeNS,
    Move,
    Pointer,
    IBeam,
}

impl CursorShape {
    pub const ALL: [CursorShape; 7] = [
        Self::CellCross,
        Self::FillCrosshair,
        Self::ResizeEW,
        Self::ResizeNS,
        Self::Move,
        Self::Pointer,
        Self::IBeam,
    ];

    pub fn sprite_id(self) -> u32 {
        SPRITE_BASE_ID + self as u32
    }

    pub fn css_name(self) -> &'static str {
        match self {
            Self::CellCross => "cell",
            Self::FillCrosshair => "crosshair",
            Self::ResizeEW => "ew-resize",
            Self::ResizeNS => "ns-resize",
            Self::Move => "move",
            Self::Pointer => "pointer",
            Self::IBeam => "text",
        }
    }

    pub fn fallback_glyph(self) -> &'static str {
        match self {
            Self::CellCross => "✚",
            Self::FillCrosshair => "┼",
            Self::ResizeEW => "↔",
            Self::ResizeNS => "↕",
            Self::Move => "✥",
            Self::Pointer => "↖",
            Self::IBeam => "▏",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CursorSprite {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    pub rgba: Vec<u8>,
}

/// Generates procedural crisp RGBA cursor sprites, including the classic Excel-style
/// fat white-filled black-bordered cross (`CellCross`) with a soft drop shadow.
pub fn build_cursor_sprite(shape: CursorShape, size: u32) -> CursorSprite {
    let sz = (size.clamp(20, 36) | 1) as i32; // odd dimension so center pixel is exact
    let c = sz / 2;
    let mut buf = vec![0u8; (sz * sz * 4) as usize];

    let mut put = |x: i32, y: i32, rgba: [u8; 4]| {
        if x >= 0 && x < sz && y >= 0 && y < sz {
            let idx = ((y * sz + x) * 4) as usize;
            buf[idx..idx + 4].copy_from_slice(&rgba);
        }
    };

    let black = [14, 18, 26, 255];
    let white = [255, 255, 255, 255];
    let shadow = [0, 0, 0, 88];
    let accent_shadow = [0, 0, 0, 64];

    match shape {
        CursorShape::CellCross => {
            let arm = (c - 2).max(7);
            let inner = 2; // 5px fat white interior
            let outer = inner + 1; // 1px crisp black border

            let in_cross = |dx: i32, dy: i32, half_w: i32, len: i32| -> bool {
                (dx.abs() <= half_w && dy.abs() <= len) || (dy.abs() <= half_w && dx.abs() <= len)
            };

            for y in 0..sz {
                for x in 0..sz {
                    let dx = x - c;
                    let dy = y - c;
                    if in_cross(dx, dy, inner, arm - 1) {
                        put(x, y, white);
                    } else if in_cross(dx, dy, outer, arm) {
                        put(x, y, black);
                    } else if in_cross(dx - 1, dy - 1, outer, arm) {
                        put(x, y, shadow);
                    }
                }
            }
            return CursorSprite {
                width: sz as u32,
                height: sz as u32,
                hotspot_x: c as u16,
                hotspot_y: c as u16,
                rgba: buf,
            };
        }
        CursorShape::FillCrosshair => {
            let arm = (c - 3).max(6);
            for d in -arm..=arm {
                for off in [-1, 1] {
                    put(c + d, c + off, [255, 255, 255, 210]);
                    put(c + off, c + d, [255, 255, 255, 210]);
                }
            }
            for d in -arm..=arm {
                put(c + d, c, black);
                put(c, c + d, black);
            }
        }
        CursorShape::ResizeEW => {
            let span = (c - 2).max(7);
            for dy in -5..=5 {
                put(c - 1, c + dy, white);
                put(c + 1, c + dy, white);
                put(c, c + dy, black);
            }
            for dx in -span..=span {
                put(c + dx, c - 1, black);
                put(c + dx, c + 1, black);
                put(c + dx, c, white);
            }
            for step in 0..=4 {
                for dy in -step..=step {
                    let lx = c - span + step;
                    let rx = c + span - step;
                    let col = if dy.abs() == step || step == 4 {
                        black
                    } else {
                        white
                    };
                    put(lx, c + dy, col);
                    put(rx, c + dy, col);
                }
            }
        }
        CursorShape::ResizeNS => {
            let span = (c - 2).max(7);
            for dx in -5..=5 {
                put(c + dx, c - 1, white);
                put(c + dx, c + 1, white);
                put(c + dx, c, black);
            }
            for dy in -span..=span {
                put(c - 1, c + dy, black);
                put(c + 1, c + dy, black);
                put(c, c + dy, white);
            }
            for step in 0..=4 {
                for dx in -step..=step {
                    let ty = c - span + step;
                    let by = c + span - step;
                    let col = if dx.abs() == step || step == 4 {
                        black
                    } else {
                        white
                    };
                    put(c + dx, ty, col);
                    put(c + dx, by, col);
                }
            }
        }
        CursorShape::Move => {
            let span = (c - 2).max(7);
            for d in -span..=span {
                put(c + d, c - 1, black);
                put(c + d, c + 1, black);
                put(c - 1, c + d, black);
                put(c + 1, c + d, black);
            }
            for d in -(span - 1)..=(span - 1) {
                put(c + d, c, white);
                put(c, c + d, white);
            }
            for step in 0i32..=3 {
                for k in -step..=step {
                    let edge = k.abs() == step || step == 3;
                    let col = if edge { black } else { white };
                    put(c - span + step, c + k, col);
                    put(c + span - step, c + k, col);
                    put(c + k, c - span + step, col);
                    put(c + k, c + span - step, col);
                }
            }
        }
        CursorShape::Pointer => {
            let h = (sz - 4).min(19);
            for y in 0..h {
                let row_w = ((y * 3) / 4).min(11);
                for x in 0..=row_w {
                    if y >= 13 && x > 6 {
                        continue;
                    }
                    let is_border = x == 0 || x == row_w || y == h - 1 || (y == 12 && x >= 5);
                    put(2 + x + 1, 2 + y + 1, accent_shadow);
                    put(2 + x, 2 + y, if is_border { black } else { white });
                }
            }
            return CursorSprite {
                width: sz as u32,
                height: sz as u32,
                hotspot_x: 2,
                hotspot_y: 2,
                rgba: buf,
            };
        }
        CursorShape::IBeam => {
            let half_h = (c - 3).max(6);
            for dy in -half_h..=half_h {
                put(c - 1, c + dy, [255, 255, 255, 200]);
                put(c + 1, c + dy, [255, 255, 255, 200]);
                put(c, c + dy, black);
            }
            for dx in -3..=3 {
                put(c + dx, c - half_h, black);
                put(c + dx, c + half_h, black);
            }
        }
    }

    CursorSprite {
        width: sz as u32,
        height: sz as u32,
        hotspot_x: c as u16,
        hotspot_y: c as u16,
        rgba: buf,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalCursorCaps {
    pub is_tty: bool,
    pub sgr_pixels: bool,
    pub osc22_none: bool,
}

pub struct SoftwareCursorRuntime {
    pub sgr_pixels_active: bool,
    is_tty: bool,
    kitty_sprites_active: bool,
    can_hide_os_cursor: bool,
    sprites_uploaded: bool,
    os_cursor_hidden: bool,
    active_sprite: Option<CursorShape>,
    last_osc22: Option<&'static str>,
    hotspots: [(u16, u16); 7],
}

impl SoftwareCursorRuntime {
    pub fn new(picker: &Picker, sgr_pixels_active: bool) -> Self {
        Self::with_caps(
            picker,
            TerminalCursorCaps {
                is_tty: false,
                sgr_pixels: sgr_pixels_active,
                osc22_none: false,
            },
        )
    }

    pub fn with_caps(picker: &Picker, caps: TerminalCursorCaps) -> Self {
        // Kitty 0.50 ignores `OSC 22;none`; only draw RGBA sprites when OS pointer actually hides.
        let kitty_sprites_active = caps.is_tty
            && caps.osc22_none
            && picker.protocol_type() == ProtocolType::Kitty
            && std::env::var_os("TMUX").is_none();
        let sprite_sz = ((picker.font_size().height.max(16) as u32) * 4 / 3).clamp(21, 35);
        let mut hotspots = [(0u16, 0u16); 7];
        for (i, shape) in CursorShape::ALL.iter().enumerate() {
            let sp = build_cursor_sprite(*shape, sprite_sz);
            hotspots[i] = (sp.hotspot_x, sp.hotspot_y);
        }
        Self {
            sgr_pixels_active: caps.sgr_pixels,
            is_tty: caps.is_tty,
            kitty_sprites_active,
            can_hide_os_cursor: caps.osc22_none,
            sprites_uploaded: false,
            os_cursor_hidden: false,
            active_sprite: None,
            last_osc22: None,
            hotspots,
        }
    }

    /// Converts an incoming `crossterm` mouse coordinate (`(col, row)` in cells or pixels)
    /// into `(cell_col, cell_row, px_x, px_y)`.
    pub fn normalize_mouse_coords(
        &self,
        raw_col: u16,
        raw_row: u16,
        font_w: u16,
        font_h: u16,
    ) -> (u16, u16, u16, u16) {
        let fw = font_w.max(1);
        let fh = font_h.max(1);
        if self.sgr_pixels_active {
            let cell_col = raw_col / fw;
            let cell_row = raw_row / fh;
            (cell_col, cell_row, raw_col, raw_row)
        } else {
            let px_x = raw_col.saturating_mul(fw).saturating_add(fw / 2);
            let px_y = raw_row.saturating_mul(fh).saturating_add(fh / 2);
            (raw_col, raw_row, px_x, px_y)
        }
    }

    /// Renders cell-glyph software cursor fallback onto Ratatui's buffer when headless (`!is_tty`)
    /// or when the terminal can hide its OS pointer (`can_hide_os_cursor`) without pixel sprites.
    pub fn draw_cell_fallback(
        &self,
        buf: &mut Buffer,
        screen: Rect,
        mouse_cell: Option<(u16, u16)>,
        shape: CursorShape,
    ) {
        if self.kitty_sprites_active || (self.is_tty && !self.can_hide_os_cursor) {
            return;
        }
        let Some((mx, my)) = mouse_cell else {
            return;
        };
        if mx < screen.x || mx >= screen.right() || my < screen.y || my >= screen.bottom() {
            return;
        }
        let style = match shape {
            CursorShape::CellCross => Style::default()
                .bg(Color::Rgb(248, 250, 252))
                .fg(Color::Rgb(15, 23, 42))
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .bg(Color::Rgb(226, 232, 240))
                .fg(Color::Rgb(15, 23, 42))
                .add_modifier(Modifier::BOLD),
        };
        buf.set_string(mx, my, shape.fallback_glyph(), style);
    }

    /// Emits post-frame pixel sprite placement or OSC 22 CSS cursor shape updates.
    pub fn sync_terminal_cursor<W: Write>(
        &mut self,
        out: &mut W,
        picker: &Picker,
        screen: Rect,
        mouse_px: Option<(u16, u16)>,
        shape: CursorShape,
    ) -> io::Result<()> {
        if !self.is_tty {
            return Ok(());
        }

        if self.kitty_sprites_active {
            if !self.sprites_uploaded {
                let sz = ((picker.font_size().height.max(16) as u32) * 4 / 3).clamp(21, 35);
                for s in CursorShape::ALL {
                    let sp = build_cursor_sprite(s, sz);
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&sp.rgba);
                    write!(
                        out,
                        "\x1b_Ga=t,f=32,s={},v={},i={},q=2;{b64}\x1b\\",
                        sp.width,
                        sp.height,
                        s.sprite_id()
                    )?;
                }
                self.sprites_uploaded = true;
            }
            if !self.os_cursor_hidden {
                write!(out, "\x1b]22;none\x1b\\")?;
                self.os_cursor_hidden = true;
                self.last_osc22 = None;
            }

            let Some((px_x, px_y)) = mouse_px else {
                out.flush()?;
                return Ok(());
            };
            let fs = picker.font_size();
            let fw = fs.width.max(1);
            let fh = fs.height.max(1);
            let (hx, hy) = self.hotspots[shape as usize];
            let tl_x = px_x.saturating_sub(hx);
            let tl_y = px_y.saturating_sub(hy);
            let col = (tl_x / fw).min(screen.right().saturating_sub(1));
            let row = (tl_y / fh).min(screen.bottom().saturating_sub(1));
            let sub_x = tl_x % fw;
            let sub_y = tl_y % fh;

            if let Some(prev) = self.active_sprite
                && prev != shape
            {
                write!(
                    out,
                    "\x1b_Ga=d,d=i,i={},p={PLACEMENT_ID},q=2\x1b\\",
                    prev.sprite_id()
                )?;
            }
            // Save/restore text cursor (\x1b7 / \x1b8) so formula/modal editing cursor never jumps.
            write!(
                out,
                "\x1b7\x1b[{};{}H\x1b_Ga=p,i={},p={PLACEMENT_ID},X={sub_x},Y={sub_y},C=1,z=100,q=2\x1b\\\x1b8",
                row + 1,
                col + 1,
                shape.sprite_id()
            )?;
            self.active_sprite = Some(shape);
            out.flush()?;
        } else if self.can_hide_os_cursor {
            if !self.os_cursor_hidden {
                write!(out, "\x1b]22;none\x1b\\")?;
                self.os_cursor_hidden = true;
                self.last_osc22 = None;
                out.flush()?;
            }
        } else {
            let css = shape.css_name();
            if self.last_osc22 != Some(css) {
                write!(out, "\x1b]22;{css}\x1b\\")?;
                self.last_osc22 = Some(css);
                out.flush()?;
            }
        }
        Ok(())
    }

    fn hide_active_sprite<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        if let Some(prev) = self.active_sprite.take() {
            write!(
                out,
                "\x1b_Ga=d,d=i,i={},p={PLACEMENT_ID},q=2\x1b\\",
                prev.sprite_id()
            )?;
        }
        Ok(())
    }

    pub fn shutdown<W: Write>(&mut self, out: &mut W) {
        if !self.is_tty {
            return;
        }
        let _ = self.hide_active_sprite(out);
        if self.sprites_uploaded {
            for s in CursorShape::ALL {
                let _ = write!(out, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", s.sprite_id());
            }
            self.sprites_uploaded = false;
        }
        if self.sgr_pixels_active {
            let _ = write!(out, "\x1b[?1016l");
            self.sgr_pixels_active = false;
        }
        let _ = write!(out, "\x1b]22;\x1b\\");
        let _ = out.flush();
    }
}

/// Raw escape cleanup safe to invoke from a panic hook.
pub fn emergency_cursor_cleanup<W: Write>(out: &mut W) {
    let _ = write!(out, "\x1b[<u\x1b[?1016l\x1b]22;\x1b\\");
    for s in CursorShape::ALL {
        let _ = write!(out, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", s.sprite_id());
    }
    let _ = out.flush();
}

pub fn has_da1_terminator(buf: &[u8]) -> bool {
    if buf.last() != Some(&b'c') {
        return false;
    }
    let mut i = buf.len() - 1;
    while i > 0 && (buf[i - 1].is_ascii_digit() || buf[i - 1] == b';') {
        i -= 1;
    }
    i >= 3 && i < buf.len() - 1 && &buf[i - 3..i] == b"\x1b[?"
}

pub fn parse_osc22_none_reply(reply: &[u8]) -> bool {
    let s = String::from_utf8_lossy(reply);
    s.contains("\x1b]22;1\x1b\\") || s.contains("\x1b]22;1\x07")
}

#[cfg(unix)]
fn query_tty_until_da1(query: &[u8]) -> Option<Vec<u8>> {
    let mut out = io::stdout();
    if out.write_all(query).is_err() || out.flush().is_err() {
        return None;
    }
    let mut reply = Vec::with_capacity(48);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(45);
    while std::time::Instant::now() < deadline {
        let rem_ms = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis() as i32;
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, rem_ms.max(1)) };
        if rc <= 0 || (pfd.revents & libc::POLLIN) == 0 {
            break;
        }
        let mut byte = [0u8; 1];
        let n = unsafe { libc::read(libc::STDIN_FILENO, byte.as_mut_ptr().cast(), 1) };
        if n <= 0 {
            break;
        }
        reply.push(byte[0]);
        if has_da1_terminator(&reply) {
            break;
        }
    }
    Some(reply)
}

/// Probes whether the terminal supports hiding the OS pointer via `OSC 22;none` (`\x1b]22;?none\x1b\\`),
/// and enables `?1016h` SGR-Pixels only when sub-cell Kitty RGBA cursor sprites will be used.
#[cfg(unix)]
pub fn probe_terminal_cursor_caps(picker: &Picker) -> TerminalCursorCaps {
    let is_tty = io::stdout().is_terminal();
    if !is_tty || !io::stdin().is_terminal() || std::env::var_os("TMUX").is_some() {
        return TerminalCursorCaps {
            is_tty,
            sgr_pixels: false,
            osc22_none: false,
        };
    }

    let osc22_none = query_tty_until_da1(b"\x1b]22;?none\x1b\\\x1b[c")
        .map(|r| parse_osc22_none_reply(&r))
        .unwrap_or(false);

    let mut sgr_pixels = false;
    if osc22_none
        && picker.protocol_type() == ProtocolType::Kitty
        && let Some(r) = query_tty_until_da1(b"\x1b[?1016h\x1b[?1016$p\x1b[c")
    {
        let s = String::from_utf8_lossy(&r);
        sgr_pixels = s.contains("?1016;1$y") || s.contains("?1016;3$y");
        if !sgr_pixels {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x1b[?1016l");
            let _ = out.flush();
        }
    }

    TerminalCursorCaps {
        is_tty: true,
        sgr_pixels,
        osc22_none,
    }
}

#[cfg(not(unix))]
pub fn probe_terminal_cursor_caps(_picker: &Picker) -> TerminalCursorCaps {
    TerminalCursorCaps {
        is_tty: io::stdout().is_terminal(),
        sgr_pixels: false,
        osc22_none: false,
    }
}
