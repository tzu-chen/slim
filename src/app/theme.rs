//! Colour themes and UI symbols. Every state shown in colour is also shown
//! by a symbol or word, so nothing depends on colour alone. ASCII mode only
//! changes UI chrome; mathematical text is always shown as written.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeName {
    /// The lean-lang.org palette, dark variant (see [`LEAN_DARK`]).
    LeanDark,
    /// The lean-lang.org palette, light variant (see [`LEAN_LIGHT`]).
    LeanLight,
    Dark,
    Light,
    Contrast,
    Mono,
}

impl ThemeName {
    /// `lean` picks the dark or light Lean theme from `COLORFGBG` (set by
    /// many terminals), defaulting to dark.
    pub fn parse(s: &str) -> Option<ThemeName> {
        match s {
            "lean" | "auto" => Some(lean_auto(std::env::var("COLORFGBG").ok().as_deref())),
            "lean-dark" => Some(ThemeName::LeanDark),
            "lean-light" => Some(ThemeName::LeanLight),
            "dark" => Some(ThemeName::Dark),
            "light" => Some(ThemeName::Light),
            "contrast" | "high-contrast" | "hc" => Some(ThemeName::Contrast),
            "mono" | "none" => Some(ThemeName::Mono),
            _ => None,
        }
    }
    pub fn next(self) -> ThemeName {
        match self {
            ThemeName::LeanDark => ThemeName::LeanLight,
            ThemeName::LeanLight => ThemeName::Dark,
            ThemeName::Dark => ThemeName::Light,
            ThemeName::Light => ThemeName::Contrast,
            ThemeName::Contrast => ThemeName::Mono,
            ThemeName::Mono => ThemeName::LeanDark,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ThemeName::LeanDark => "Lean dark (lean-lang.org palette)",
            ThemeName::LeanLight => "Lean light (lean-lang.org palette)",
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
            ThemeName::Contrast => "high contrast",
            ThemeName::Mono => "monochrome",
        }
    }
}

/// `COLORFGBG` is `"fg;bg"` (sometimes `"fg;default;bg"`) with ANSI colour
/// numbers; backgrounds 7 and 15 (white) mean a light terminal.
pub fn lean_auto(colorfgbg: Option<&str>) -> ThemeName {
    let bg = colorfgbg.and_then(|v| v.rsplit(';').next()).and_then(|b| b.trim().parse::<u8>().ok());
    match bg {
        Some(7) | Some(15) => ThemeName::LeanLight,
        _ => ThemeName::LeanDark,
    }
}

/// A theme palette as hex strings, so the colours can be documented and
/// their contrast checked in tests.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: &'static str,
    pub text: &'static str,
    pub dim: &'static str,
    pub accent: &'static str,
    pub heading: &'static str,
    pub selected_fg: &'static str,
    pub selected_bg: &'static str,
    pub unfocused_fg: &'static str,
    pub unfocused_bg: &'static str,
    pub border: &'static str,
    pub border_focus: &'static str,
    pub warn: &'static str,
    pub danger: &'static str,
    pub ok: &'static str,
    pub code: &'static str,
    pub decl_name: &'static str,
    pub kind: &'static str,
    pub tag_type: &'static str,
    pub tag_value: &'static str,
    pub tag_ctor: &'static str,
    pub bar_fg: &'static str,
    pub bar_bg: &'static str,
    pub input: &'static str,
    pub highlight_bg: &'static str,
}

/// lean-lang.org light palette (checked 2026-11-10). Site variables from the
/// page's `:root` block: `--color-bg #F9FBFD`, `--color-text #333`,
/// `--color-muted #5F6B78`, `--color-primary #386EE0`,
/// `--color-primary-focus #1D4ED8`, `--color-border-light #D1D9E2`,
/// `--color-border #E1E7F0`, `--color-text-contrast white`, gradient tint
/// `#D5E1FF`. Lean code colours from `-verso-data/lean-themed.css`: keyword
/// `#8b21b8`, comment `#2f7d4f`, tactic-state text `#24292f`. Adjusted only
/// in lightness (same hue) to reach WCAG AA (4.5:1) on `#F9FBFD`: constant
/// `#267f99`→`#247a93`, literal `#098658`→`#098155`, warning indicator
/// `#e7a71d`→`#91680f`, error indicator `#ff0000`→`#e30000`. The
/// highlighted-line background `#EAF0FB` is not a site colour: the background
/// with about 8 % of the primary blue.
pub const LEAN_LIGHT: Palette = Palette {
    bg: "#F9FBFD",
    text: "#333333",
    dim: "#5F6B78",
    accent: "#386EE0",
    heading: "#1D4ED8",
    selected_fg: "#FFFFFF",
    selected_bg: "#386EE0",
    unfocused_fg: "#333333",
    unfocused_bg: "#D5E1FF",
    border: "#D1D9E2",
    border_focus: "#386EE0",
    warn: "#91680F",
    danger: "#E30000",
    ok: "#2F7D4F",
    code: "#24292F",
    decl_name: "#247A93",
    kind: "#8B21B8",
    tag_type: "#386EE0",
    tag_value: "#098155",
    tag_ctor: "#8B21B8",
    bar_fg: "#333333",
    bar_bg: "#E1E7F0",
    input: "#1D4ED8",
    highlight_bg: "#EAF0FB",
};

/// lean-lang.org dark palette (checked 2026-11-10). Site variables from the
/// page's `.dark-theme` block: `--color-bg #181818`, `--color-surface #121212`,
/// `--color-text #eee`, `--color-muted #90a4ae`, `--color-primary #3b94ff`,
/// `--color-primary-light #6aadfe`, `--color-border-light #444`. Lean code
/// colours from `-verso-data/lean-dark.css`: text `#d4d4d4`, constant
/// `#4ec9b0`, keyword `#c586c0`, literal `#b5cea8`, comment `#6a9955`,
/// code-block navy `#2c3346`/`#232838`, warning indicator `#e7a71d`. Adjusted
/// only in lightness for WCAG AA on `#181818`: error indicator
/// `#ff0000`→`#ff4545`. Selection text is the dark surface colour because
/// white on `#3b94ff` is only 3.1:1. The highlighted-line background
/// `#2B2B2B` is not a site colour: the background lifted about 9 % towards
/// the text colour.
pub const LEAN_DARK: Palette = Palette {
    bg: "#181818",
    text: "#EEEEEE",
    dim: "#90A4AE",
    accent: "#3B94FF",
    heading: "#6AADFE",
    selected_fg: "#121212",
    selected_bg: "#3B94FF",
    unfocused_fg: "#EEEEEE",
    unfocused_bg: "#2C3346",
    border: "#444444",
    border_focus: "#3B94FF",
    warn: "#E7A71D",
    danger: "#FF4545",
    ok: "#6A9955",
    code: "#D4D4D4",
    decl_name: "#4EC9B0",
    kind: "#C586C0",
    tag_type: "#3B94FF",
    tag_value: "#B5CEA8",
    tag_ctor: "#C586C0",
    bar_fg: "#EEEEEE",
    bar_bg: "#232838",
    input: "#6AADFE",
    highlight_bg: "#2B2B2B",
};

pub fn hex_rgb(h: &str) -> (u8, u8, u8) {
    let h = h.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    (p(0), p(2), p(4))
}

/// Whether the terminal advertises 24-bit colour (`COLORTERM`), overridable
/// with `SLIM_TRUECOLOR=1|0`.
pub fn truecolor() -> bool {
    match std::env::var("SLIM_TRUECOLOR").ok().as_deref() {
        Some("1") | Some("yes") | Some("true") => return true,
        Some("0") | Some("no") | Some("false") => return false,
        _ => {}
    }
    matches!(std::env::var("COLORTERM").ok().as_deref(), Some("truecolor") | Some("24bit"))
}

/// Nearest xterm-256 colour (6×6×6 cube or grey ramp) to an RGB value.
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let near = |v: u8| -> usize {
        let v = v as i32;
        (0..6).min_by_key(|&i| (LEVELS[i] - v).abs()).unwrap()
    };
    let (ri, gi, bi) = (near(r), near(g), near(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let d = |a: (i32, i32, i32)| {
        let (x, y, z) = (a.0 - r as i32, a.1 - g as i32, a.2 - b as i32);
        x * x + y * y + z * z
    };
    let cube_d = d((LEVELS[ri], LEVELS[gi], LEVELS[bi]));
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gi2 = (((avg - 8) as f64 / 10.0).round() as i32).clamp(0, 23);
    let gv = 8 + 10 * gi2;
    let grey_d = d((gv, gv, gv));
    if grey_d < cube_d {
        (232 + gi2) as u8
    } else {
        cube as u8
    }
}

/// RGB of an xterm-256 colour from the cube or the grey ramp (16–255; the
/// 16 system colours are terminal-defined and never chosen here).
pub fn xterm_rgb(i: u8) -> (u8, u8, u8) {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    if i >= 232 {
        let v = 8 + 10 * (i - 232);
        (v, v, v)
    } else {
        let i = i.max(16) - 16;
        (LEVELS[(i / 36) as usize], LEVELS[(i / 6 % 6) as usize], LEVELS[(i % 6) as usize])
    }
}

/// WCAG 2 contrast ratio of two sRGB colours (1 to 21).
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let lum = |(r, g, b): (u8, u8, u8)| {
        let f = |c: u8| {
            let c = c as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// [`nearest_256`] for text drawn on `bg`: the nearest colour that keeps
/// 4.5:1 against `bg`'s own approximation, so the fallback stays as readable
/// as the palette (the plain nearest colour if none does).
pub fn nearest_256_on(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> u8 {
    let (r, g, b) = fg;
    let plain = nearest_256(r, g, b);
    let back = xterm_rgb(nearest_256(bg.0, bg.1, bg.2));
    if contrast(xterm_rgb(plain), back) >= 4.5 {
        return plain;
    }
    let d = |(x, y, z): (u8, u8, u8)| (x as i32 - r as i32).pow(2) + (y as i32 - g as i32).pow(2) + (z as i32 - b as i32).pow(2);
    (16..=255u8).filter(|&i| contrast(xterm_rgb(i), back) >= 4.5).min_by_key(|&i| d(xterm_rgb(i))).unwrap_or(plain)
}

fn color(h: &str, tc: bool) -> Color {
    let (r, g, b) = hex_rgb(h);
    if tc {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(nearest_256(r, g, b))
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: ThemeName,
    /// Painted over the whole frame first; `Style::default()` keeps the
    /// terminal's own colours (the classic themes), the Lean themes set
    /// their background so they read correctly in any terminal.
    pub base: Style,
    pub text: Style,
    pub dim: Style,
    pub accent: Style,
    pub heading: Style,
    pub title: Style,
    /// The active tab of the right pane.
    pub tab_active: Style,
    pub selected: Style,
    pub selected_unfocused: Style,
    pub border: Style,
    pub border_focus: Style,
    pub warn: Style,
    pub danger: Style,
    pub ok: Style,
    pub code: Style,
    pub decl_name: Style,
    pub kind: Style,
    pub tag_type: Style,
    pub tag_value: Style,
    pub tag_ctor: Style,
    pub bar: Style,
    pub input: Style,
    pub highlight_line: Style,
    /// What syntax colours have to work with in this theme.
    pub syntax: SyntaxEnv,
}

/// The terminal side of syntax colouring: colour depth and, where the theme
/// paints its own, the backgrounds text must stay legible on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxEnv {
    pub truecolor: bool,
    /// No colours at all: only bold, italic and underline carry over.
    pub mono: bool,
    pub bg: Option<(u8, u8, u8)>,
    /// Background of the highlighted (current declaration) lines.
    pub line_bg: Option<(u8, u8, u8)>,
}

/// Below this contrast against its background a syntax colour is dropped
/// in favour of the view's own text colour.
pub const MIN_SYNTAX_CONTRAST: f64 = 3.0;

impl Theme {
    /// The bundled syntax theme that goes with this UI theme.
    pub fn default_syntax_theme(&self) -> &'static str {
        match self.name {
            ThemeName::LeanDark | ThemeName::Dark | ThemeName::Mono => "dark-plus",
            ThemeName::LeanLight | ThemeName::Light => "light-plus",
            ThemeName::Contrast => "hc-black",
        }
    }

    /// A logical token style over `base`. `on_line` says the text sits on
    /// the highlighted-line background. Compositing order: base text style,
    /// then the token's colour and font style; overlay backgrounds stay.
    pub fn tok_style(&self, base: Style, tok: crate::highlight::theme::TokStyle, on_line: bool) -> Style {
        use crate::highlight::theme as st;
        let mut out = base;
        let env = self.syntax;
        if let (Some(c), false) = (tok.fg, env.mono) {
            let fg = (c[0], c[1], c[2]);
            let bg = if on_line { env.line_bg.or(env.bg) } else { env.bg };
            if bg.is_none_or(|b| contrast(fg, b) >= MIN_SYNTAX_CONTRAST) {
                out = out.fg(if env.truecolor {
                    Color::Rgb(fg.0, fg.1, fg.2)
                } else {
                    Color::Indexed(match bg {
                        Some(b) => nearest_256_on(fg, b),
                        None => nearest_256(fg.0, fg.1, fg.2),
                    })
                });
            }
        }
        for (bit, m) in [(st::BOLD, Modifier::BOLD), (st::ITALIC, Modifier::ITALIC), (st::UNDERLINE, Modifier::UNDERLINED), (st::STRIKE, Modifier::CROSSED_OUT)] {
            if tok.font & bit != 0 {
                out = out.add_modifier(m);
            }
        }
        out
    }

    pub fn from_palette(name: ThemeName, p: &Palette, tc: bool) -> Theme {
        let c = |h: &str| color(h, tc);
        // Text colour `h` drawn on `bg`; see `nearest_256_on`.
        let on = |h: &str, bg: &str| if tc { c(h) } else { Color::Indexed(nearest_256_on(hex_rgb(h), hex_rgb(bg))) };
        let fg = |h: &str| Style::default().fg(on(h, p.bg));
        let bold = Modifier::BOLD;
        let selected = Style::default().bg(c(p.selected_bg)).fg(on(p.selected_fg, p.selected_bg)).add_modifier(bold);
        Theme {
            name,
            base: Style::default().bg(c(p.bg)).fg(on(p.text, p.bg)),
            text: fg(p.text),
            dim: fg(p.dim),
            accent: fg(p.accent),
            heading: fg(p.heading).add_modifier(bold),
            title: fg(p.text).add_modifier(bold),
            tab_active: selected,
            selected,
            selected_unfocused: Style::default().bg(c(p.unfocused_bg)).fg(on(p.unfocused_fg, p.unfocused_bg)),
            // Borders are decoration, not text: plain nearest colour.
            border: Style::default().fg(c(p.border)),
            border_focus: fg(p.border_focus),
            warn: fg(p.warn).add_modifier(bold),
            danger: fg(p.danger).add_modifier(bold),
            ok: fg(p.ok),
            code: fg(p.code),
            decl_name: fg(p.decl_name).add_modifier(bold),
            kind: fg(p.kind),
            tag_type: fg(p.tag_type),
            tag_value: fg(p.tag_value),
            tag_ctor: fg(p.tag_ctor),
            bar: Style::default().bg(c(p.bar_bg)).fg(on(p.bar_fg, p.bar_bg)),
            input: fg(p.input),
            highlight_line: Style::default().bg(c(p.highlight_bg)),
            syntax: SyntaxEnv { truecolor: tc, mono: false, bg: Some(hex_rgb(p.bg)), line_bg: Some(hex_rgb(p.highlight_bg)) },
        }
    }

    pub fn new(name: ThemeName) -> Theme {
        let s = Style::default;
        match name {
            ThemeName::LeanDark => Theme::from_palette(name, &LEAN_DARK, truecolor()),
            ThemeName::LeanLight => Theme::from_palette(name, &LEAN_LIGHT, truecolor()),
            ThemeName::Dark => Theme {
                name,
                base: s(),
                text: s(),
                dim: s().fg(Color::DarkGray),
                accent: s().fg(Color::Cyan),
                heading: s().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                title: s().add_modifier(Modifier::BOLD),
                tab_active: s().add_modifier(Modifier::BOLD).add_modifier(Modifier::REVERSED),
                selected: s().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD),
                selected_unfocused: s().add_modifier(Modifier::REVERSED),
                border: s().fg(Color::DarkGray),
                border_focus: s().fg(Color::Cyan),
                warn: s().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                danger: s().fg(Color::Red).add_modifier(Modifier::BOLD),
                ok: s().fg(Color::Green),
                code: s().fg(Color::White),
                decl_name: s().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
                kind: s().fg(Color::Magenta),
                tag_type: s().fg(Color::LightBlue),
                tag_value: s().fg(Color::LightGreen),
                tag_ctor: s().fg(Color::LightMagenta),
                bar: s().bg(Color::DarkGray).fg(Color::White),
                input: s().fg(Color::Yellow),
                highlight_line: s().bg(Color::Rgb(40, 40, 60)),
                syntax: SyntaxEnv { truecolor: truecolor(), mono: false, bg: None, line_bg: None },
            },
            ThemeName::Light => Theme {
                name,
                base: s(),
                text: s().fg(Color::Black),
                dim: s().fg(Color::DarkGray),
                accent: s().fg(Color::Blue),
                heading: s().fg(Color::Blue).add_modifier(Modifier::BOLD),
                title: s().fg(Color::Black).add_modifier(Modifier::BOLD),
                tab_active: s().fg(Color::Black).add_modifier(Modifier::BOLD).add_modifier(Modifier::REVERSED),
                selected: s().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD),
                selected_unfocused: s().add_modifier(Modifier::REVERSED),
                border: s().fg(Color::Gray),
                border_focus: s().fg(Color::Blue),
                warn: s().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                danger: s().fg(Color::Red).add_modifier(Modifier::BOLD),
                ok: s().fg(Color::Green),
                code: s().fg(Color::Black),
                decl_name: s().fg(Color::Blue).add_modifier(Modifier::BOLD),
                kind: s().fg(Color::Magenta),
                tag_type: s().fg(Color::Blue),
                tag_value: s().fg(Color::Green),
                tag_ctor: s().fg(Color::Magenta),
                bar: s().bg(Color::Gray).fg(Color::Black),
                input: s().fg(Color::Blue),
                highlight_line: s().bg(Color::Rgb(225, 230, 250)),
                syntax: SyntaxEnv { truecolor: truecolor(), mono: false, bg: None, line_bg: None },
            },
            ThemeName::Contrast => Theme {
                name,
                base: s(),
                text: s().fg(Color::White),
                dim: s().fg(Color::White),
                accent: s().fg(Color::White).add_modifier(Modifier::BOLD),
                heading: s().fg(Color::White).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                title: s().fg(Color::White).add_modifier(Modifier::BOLD),
                tab_active: s().fg(Color::White).add_modifier(Modifier::BOLD).add_modifier(Modifier::REVERSED),
                selected: s().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD),
                selected_unfocused: s().add_modifier(Modifier::REVERSED),
                border: s().fg(Color::White),
                border_focus: s().fg(Color::White).add_modifier(Modifier::BOLD),
                warn: s().fg(Color::White).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                danger: s().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD),
                ok: s().fg(Color::White),
                code: s().fg(Color::White),
                decl_name: s().fg(Color::White).add_modifier(Modifier::BOLD),
                kind: s().fg(Color::White),
                tag_type: s().fg(Color::White),
                tag_value: s().fg(Color::White),
                tag_ctor: s().fg(Color::White),
                bar: s().bg(Color::White).fg(Color::Black),
                input: s().fg(Color::White).add_modifier(Modifier::BOLD),
                highlight_line: s().add_modifier(Modifier::BOLD),
                syntax: SyntaxEnv { truecolor: truecolor(), mono: false, bg: None, line_bg: None },
            },
            ThemeName::Mono => Theme {
                name,
                base: s(),
                text: s(),
                dim: s(),
                accent: s().add_modifier(Modifier::BOLD),
                heading: s().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                title: s().add_modifier(Modifier::BOLD),
                tab_active: s().add_modifier(Modifier::BOLD).add_modifier(Modifier::REVERSED),
                selected: s().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                selected_unfocused: s().add_modifier(Modifier::REVERSED),
                border: s(),
                border_focus: s().add_modifier(Modifier::BOLD),
                warn: s().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                danger: s().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                ok: s(),
                code: s(),
                decl_name: s().add_modifier(Modifier::BOLD),
                kind: s(),
                tag_type: s(),
                tag_value: s(),
                tag_ctor: s(),
                bar: s().add_modifier(Modifier::REVERSED),
                input: s().add_modifier(Modifier::BOLD),
                highlight_line: s().add_modifier(Modifier::BOLD),
                syntax: SyntaxEnv { truecolor: truecolor(), mono: true, bg: None, line_bg: None },
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Symbols {
    pub ok: &'static str,
    pub warn: &'static str,
    pub stop: &'static str,
    pub bullet: &'static str,
    pub arrow: &'static str,
    pub larrow: &'static str,
    pub expanded: &'static str,
    pub collapsed: &'static str,
    pub child: &'static str,
    pub cycle: &'static str,
    pub ellipsis: &'static str,
    pub sep: &'static str,
    pub crumb: &'static str,
    pub bookmark: &'static str,
    pub gutter: &'static str,
    pub border: border::Set<'static>,
    pub rule: &'static str,
    /// A node that also appears elsewhere in an unfolded tree.
    pub link: &'static str,
    pub goal: &'static str,
    pub objective: &'static str,
    pub question: &'static str,
}

pub const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

impl Symbols {
    pub fn new(ascii: bool) -> Symbols {
        if ascii {
            Symbols {
                ok: "ok",
                warn: "!!",
                stop: "XX",
                bullet: "*",
                arrow: "->",
                larrow: "<-",
                expanded: "v",
                collapsed: ">",
                child: "`-",
                cycle: "@cycle",
                ellipsis: "...",
                sep: "|",
                crumb: ">",
                bookmark: "*",
                gutter: "|",
                border: ASCII_BORDER,
                rule: "-",
                link: "^again",
                goal: "(goal)",
                objective: "#",
                question: "?",
            }
        } else {
            Symbols {
                ok: "✓",
                warn: "⚠",
                stop: "✗",
                bullet: "·",
                arrow: "→",
                larrow: "←",
                expanded: "▾",
                collapsed: "▸",
                child: "└",
                cycle: "↺ cycle",
                ellipsis: "…",
                sep: "│",
                crumb: "›",
                bookmark: "★",
                gutter: "│",
                border: border::ROUNDED,
                rule: "─",
                link: "↗ again",
                goal: "◎",
                objective: "◆",
                question: "?",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: Option<Color>) -> (u8, u8, u8) {
        match c {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            Some(Color::Indexed(i)) => xterm_rgb(i),
            other => panic!("not an RGB or indexed colour: {other:?}"),
        }
    }

    /// Every text colour of both Lean themes meets WCAG AA (4.5:1) against
    /// what it is drawn on, in 24-bit colour and in the xterm-256 fallback.
    #[test]
    fn lean_themes_meet_wcag_aa() {
        for (name, p) in [("light", LEAN_LIGHT), ("dark", LEAN_DARK)] {
            for tc in [true, false] {
                let t = Theme::from_palette(ThemeName::LeanDark, &p, tc);
                let bg = rgb(t.base.bg);
                let on_bg = [
                    ("text", t.base),
                    ("dim", t.dim),
                    ("accent", t.accent),
                    ("heading", t.heading),
                    ("title", t.title),
                    ("warn", t.warn),
                    ("danger", t.danger),
                    ("ok", t.ok),
                    ("code", t.code),
                    ("decl_name", t.decl_name),
                    ("kind", t.kind),
                    ("tag_type", t.tag_type),
                    ("tag_value", t.tag_value),
                    ("tag_ctor", t.tag_ctor),
                    ("input", t.input),
                ];
                for (role, st) in on_bg {
                    let c = contrast(rgb(st.fg), bg);
                    assert!(c >= 4.5, "{name} (24-bit: {tc}) {role} on the background: {c:.2}");
                }
                for (role, st) in [("selection", t.selected), ("active tab", t.tab_active), ("unfocused selection", t.selected_unfocused), ("bar", t.bar)] {
                    let c = contrast(rgb(st.fg), rgb(st.bg));
                    assert!(c >= 4.5, "{name} (24-bit: {tc}) {role}: {c:.2}");
                }
                let c = contrast(rgb(t.code.fg), rgb(t.highlight_line.bg));
                assert!(c >= 4.5, "{name} (24-bit: {tc}) code on the highlighted line: {c:.2}");
            }
        }
    }

    #[test]
    fn auto_follows_colorfgbg() {
        assert_eq!(lean_auto(Some("15;0")), ThemeName::LeanDark);
        assert_eq!(lean_auto(Some("0;15")), ThemeName::LeanLight);
        assert_eq!(lean_auto(Some("0;default;7")), ThemeName::LeanLight);
        assert_eq!(lean_auto(None), ThemeName::LeanDark);
        assert_eq!(lean_auto(Some("garbage")), ThemeName::LeanDark);
    }

    #[test]
    fn xterm_256_fallback() {
        assert_eq!(nearest_256(0xff, 0, 0), 196);
        assert_eq!(nearest_256(0x18, 0x18, 0x18), 234);
        assert_eq!(nearest_256(0xff, 0xff, 0xff), 231);
        let (r, g, b) = hex_rgb(LEAN_DARK.accent);
        let i = nearest_256(r, g, b);
        assert!((16..=231).contains(&i), "a blue maps into the colour cube: {i}");
        for i in 16..=255u8 {
            let (r, g, b) = xterm_rgb(i);
            assert_eq!(nearest_256(r, g, b), i, "round trip of colour {i}");
        }
        // The plain nearest colour of light decl_name (#247A93) is #008787,
        // only 4.36:1 on the light background: a darker neighbour is used.
        let light = Theme::from_palette(ThemeName::LeanLight, &LEAN_LIGHT, false);
        assert_eq!(nearest_256(0x24, 0x7A, 0x93), 30);
        assert_ne!(light.decl_name.fg, Some(Color::Indexed(30)));
    }
}
