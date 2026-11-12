//! A partner's accent, and the colours that follow from it (task 5ea48ab1).
//!
//! ## Why this is not in `theme`
//!
//! [`crate::theme`] says plainly that it does not hand a shell hex values, and that still holds:
//! it answers *which look* is chosen, and each client renders it. This answers a different
//! question — given one brand accent, what are the dependent brand colours — which is a property
//! of a partner's configuration rather than of a running screen.
//!
//! ## The problem it solves
//!
//! Each client derived them differently, so one brand looked like four. Web darkened the accent
//! 15% for a hover and had no ink at all. iOS read `BrandAccentHoverColor` and **fell back to
//! Astrid's blue**, so Acme's purple got a blue hover. Android took hover and text as raw
//! properties, also with no ink. Windows hard-coded Astrid's `#3B82F6` and its six WinUI shades
//! twenty-two times. And nothing checked a partner's accent at all: the generator's contrast
//! budget was applied to Astrid's own committed values only, so a partner could ship a yellow
//! accent with a white label on it and find out from a user.
//!
//! ## What is derived, and what is not
//!
//! Astrid's own `accent`, `accent-hover` and the inks are **hand-picked and committed** in
//! `design/tokens.json`, and deriving them is not the point: `#2563EB` → `#1D4ED8` is the next
//! step on a designed ramp, not a multiply, and no formula reproduces it. Derivation is for a
//! partner who supplies one accent and nothing else. So a stated value always wins; this fills
//! only what is missing.

use serde::{Deserialize, Serialize};

/// A colour as the clients spell it.
pub type Rgb = (u8, u8, u8);

/// The smallest contrast a label on a fill, or an ink as text, is allowed (DS25, DS26).
pub const AA_BODY: f64 = 4.5;

/// The label on an accent fill. White, in every theme — DS26.
pub const ACCENT_TEXT: Rgb = (0xFF, 0xFF, 0xFF);

/// The card an ink is read on, light and dark (DS9, `design/tokens.json`).
///
/// A partner replaces the `brand` block, not the themes, so these are Astrid's cards and are
/// constants rather than inputs. If that ever stops being true they become arguments.
const LIGHT_CARD: Rgb = (0xFF, 0xFF, 0xFF);
const DARK_CARD: Rgb = (0x1F, 0x29, 0x37);

/// How much of an ink's own colour sits under it in a wash (DS25).
const INK_WASH: f64 = 0.12;

/// Every colour that follows from one accent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandColors {
    pub accent: String,
    pub accent_hover: String,
    pub accent_ink: String,
    pub accent_ink_dark: String,
    pub accent_text: String,
    /// The six shades WinUI derives its own brushes from, darkest first is NOT the order:
    /// `light1..3` then `dark1..3`, as `SystemAccentColorLight1`… are named.
    pub winui: WinUiShades,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinUiShades {
    pub light1: String,
    pub light2: String,
    pub light3: String,
    pub dark1: String,
    pub dark2: String,
    pub dark3: String,
}

/// Why an accent cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refusal {
    pub accent: String,
    pub reason: String,
}

/// `#rrggbb` → channels. `None` for anything else, including a named colour or `rgb()`.
pub fn parse_hex(value: &str) -> Option<Rgb> {
    let body = value.trim().strip_prefix('#')?;
    if body.len() != 6 || !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((
        u8::from_str_radix(&body[0..2], 16).ok()?,
        u8::from_str_radix(&body[2..4], 16).ok()?,
        u8::from_str_radix(&body[4..6], 16).ok()?,
    ))
}

pub fn hex(colour: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", colour.0, colour.1, colour.2)
}

fn channel(value: u8) -> f64 {
    let v = f64::from(value) / 255.0;
    // 0.03928, the threshold the design generator has always used and against which every
    // committed token was checked. The later 0.04045 differs only in the eighth decimal of a
    // ratio, but changing it here would silently re-judge values somebody already signed off.
    if v <= 0.03928 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(colour: Rgb) -> f64 {
    0.2126 * channel(colour.0) + 0.7152 * channel(colour.1) + 0.0722 * channel(colour.2)
}

/// WCAG contrast between two opaque colours, highest first or not — it is symmetric.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    let (lighter, darker) = if x > y { (x, y) } else { (y, x) };
    (lighter + 0.05) / (darker + 0.05)
}

/// `top` at `alpha` over `under`, which is what a wash is.
pub fn over(top: Rgb, under: Rgb, alpha: f64) -> Rgb {
    let mix = |t: u8, u: u8| (f64::from(t) * alpha + f64::from(u) * (1.0 - alpha)).round() as u8;
    (
        mix(top.0, under.0),
        mix(top.1, under.1),
        mix(top.2, under.2),
    )
}

/// Toward black by `amount` (0–1), the way astrid-web's `darkenHex` does it.
fn darken(colour: Rgb, amount: f64) -> Rgb {
    let scale = |c: u8| (f64::from(c) * (1.0 - amount)).round().max(0.0) as u8;
    (scale(colour.0), scale(colour.1), scale(colour.2))
}

/// Toward white by `amount` (0–1).
fn lighten(colour: Rgb, amount: f64) -> Rgb {
    let scale = |c: u8| {
        (f64::from(c) + (255.0 - f64::from(c)) * amount)
            .round()
            .min(255.0) as u8
    };
    (scale(colour.0), scale(colour.1), scale(colour.2))
}

/// An ink reads on the card, and on its own wash over that card (DS25).
fn ink_is_readable(ink: Rgb, card: Rgb) -> bool {
    contrast(ink, card) >= AA_BODY && contrast(ink, over(ink, card, INK_WASH)) >= AA_BODY
}

/// Step the accent toward black (on a light card) or white (on a dark one) until it reads.
///
/// Stepping rather than solving: the budget has two clauses, one of which is the ink against a
/// wash *of itself*, so the target moves as the ink does. Twenty steps of 5% covers the range, and
/// the extremes are checked last so a colour that can never pass says so rather than looping.
fn ink_for(accent: Rgb, card: Rgb, toward_white: bool) -> Option<Rgb> {
    if ink_is_readable(accent, card) {
        return Some(accent);
    }
    for step in 1..=20 {
        let amount = f64::from(step) * 0.05;
        let candidate = if toward_white {
            lighten(accent, amount)
        } else {
            darken(accent, amount)
        };
        if ink_is_readable(candidate, card) {
            return Some(candidate);
        }
    }
    None
}

/// Every colour that follows from `accent`, or why it cannot be used.
///
/// The one refusal is the label: `accent-text` is white in every theme (DS26), so an accent that
/// white cannot be read on is not a brand accent at all. That is the check the generator applied
/// to Astrid's own values and never to a partner's, which is the hole this closes.
pub fn derive(accent: &str) -> Result<BrandColors, Refusal> {
    let refuse = |reason: String| Refusal {
        accent: accent.to_string(),
        reason,
    };

    let Some(rgb) = parse_hex(accent) else {
        return Err(refuse(format!(
            "{accent:?} is not a #rrggbb colour, so nothing can be derived from it"
        )));
    };

    let on_accent = contrast(ACCENT_TEXT, rgb);
    if on_accent < AA_BODY {
        return Err(refuse(format!(
            "white on {} is {on_accent:.2}:1, under the {AA_BODY}:1 a label needs — \
             choose a darker accent",
            hex(rgb)
        )));
    }

    // One step on, so it still reads as a hover rather than as a different colour. Web's 15%.
    let hover = darken(rgb, 0.15);

    let Some(ink) = ink_for(rgb, LIGHT_CARD, false) else {
        return Err(refuse(format!(
            "no shade of {} reads at {AA_BODY}:1 on a light card",
            hex(rgb)
        )));
    };
    let Some(ink_dark) = ink_for(rgb, DARK_CARD, true) else {
        return Err(refuse(format!(
            "no shade of {} reads at {AA_BODY}:1 on a dark card",
            hex(rgb)
        )));
    };

    Ok(BrandColors {
        accent: hex(rgb),
        accent_hover: hex(hover),
        accent_ink: hex(ink),
        accent_ink_dark: hex(ink_dark),
        accent_text: hex(ACCENT_TEXT),
        winui: WinUiShades {
            light1: hex(lighten(rgb, 0.15)),
            light2: hex(lighten(rgb, 0.30)),
            light3: hex(lighten(rgb, 0.45)),
            dark1: hex(darken(rgb, 0.15)),
            dark2: hex(darken(rgb, 0.30)),
            dark3: hex(darken(rgb, 0.45)),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The acceptance's own example. Yellow takes a black label everywhere else in the world, but
    /// `accent-text` is white in every theme (DS26) — so this is not a usable brand accent, and
    /// saying so at configuration time is the whole point.
    #[test]
    fn a_yellow_accent_is_refused_with_a_reason_5ea48ab1() {
        let refusal = derive("#FFFF00").expect_err("refused");

        assert!(refusal.reason.contains("white on #FFFF00"), "{refusal:?}");
        assert!(refusal.reason.contains("1.07:1"), "{refusal:?}");
        assert!(
            refusal.reason.contains("choose a darker accent"),
            "{refusal:?}"
        );
    }

    /// A partner's accent derives every dependent colour from itself alone.
    #[test]
    fn a_partners_accent_derives_every_dependent_colour_5ea48ab1() {
        let colours = derive("#7E22CE").expect("usable");

        assert_eq!(colours.accent, "#7E22CE");
        assert_eq!(colours.accent_text, "#FFFFFF");
        // The hover is one step on, not another colour.
        assert_eq!(colours.accent_hover, "#6B1DAF");
        assert_ne!(colours.accent_ink, colours.accent_ink_dark);
    }

    /// The bug this closes, stated as a test: a purple brand must not get Astrid's blue hover.
    #[test]
    fn a_partners_hover_is_their_own_colour_5ea48ab1() {
        let partner = derive("#7E22CE").expect("usable");
        let astrid = derive("#2563EB").expect("usable");

        assert_ne!(partner.accent_hover, astrid.accent_hover);
        assert_ne!(partner.winui.light2, astrid.winui.light2);
    }

    /// **A shipped profile fails this, and that is the finding, not a bug.**
    ///
    /// `astrid-web/brands/acme.brand.json` sets `#a855f7`, and white on it is 3.96:1 — under the
    /// 4.5:1 DS26 requires of a fill that carries a label. Nothing checked a partner's accent
    /// before, which is exactly the hole this task names, so the first thing the check does is
    /// fail an existing fixture. Acme needs a darker purple (`#7E22CE` is 6.99:1); until somebody
    /// changes it, this test records that the refusal is deliberate rather than a regression.
    #[test]
    fn acmes_shipped_accent_fails_the_label_budget_5ea48ab1() {
        let refusal = derive("#a855f7").expect_err("under 4.5:1");

        assert!(refusal.reason.contains("3.96:1"), "{refusal:?}");
    }

    /// Every ink must clear the budget it was derived against — on the card, and on its own wash
    /// over that card, which is the clause that makes this a search rather than a formula.
    #[test]
    fn every_derived_ink_meets_its_budget_5ea48ab1() {
        for accent in ["#7E22CE", "#2563EB", "#DC2626", "#15803D", "#0E7490"] {
            let colours = derive(accent).expect(accent);
            let light = parse_hex(&colours.accent_ink).expect("hex");
            let dark = parse_hex(&colours.accent_ink_dark).expect("hex");

            assert!(ink_is_readable(light, LIGHT_CARD), "{accent} light ink");
            assert!(ink_is_readable(dark, DARK_CARD), "{accent} dark ink");
        }
    }

    /// A light accent needs a LIGHTER ink on a dark card and a DARKER one on a light card, which
    /// is the asymmetry DS25 exists for — a single mid-tone fails one of the two.
    #[test]
    fn the_two_inks_move_in_opposite_directions_5ea48ab1() {
        let colours = derive("#2563EB").expect("usable");
        let accent = parse_hex("#2563EB").expect("hex");
        let light = parse_hex(&colours.accent_ink).expect("hex");
        let dark = parse_hex(&colours.accent_ink_dark).expect("hex");

        assert!(
            luminance(light) <= luminance(accent),
            "a light theme's ink must be no lighter than the accent"
        );
        assert!(
            luminance(dark) >= luminance(accent),
            "a dark theme's ink must be no darker than the accent"
        );
    }

    /// Not a colour at all: a partner who sets `purple` or `rgb(…)` gets told, rather than
    /// silently getting Astrid's blue, which is what iOS did.
    #[test]
    fn a_value_that_is_not_a_colour_is_refused_5ea48ab1() {
        for bad in ["purple", "rgb(168, 85, 247)", "#A855F", ""] {
            let refusal = derive(bad).expect_err(bad);
            assert!(
                refusal.reason.contains("not a #rrggbb"),
                "{bad}: {refusal:?}"
            );
        }
    }

    /// Derivation is reproducible from the accent alone — that is what makes "byte-identical on
    /// web, iOS, Android and Windows" a property of the code rather than a hope.
    #[test]
    fn derivation_is_reproducible_5ea48ab1() {
        assert_eq!(derive("#7E22CE"), derive("#7e22ce"));
    }

    #[test]
    fn contrast_is_symmetric_and_known_5ea48ab1() {
        let white = (0xFF, 0xFF, 0xFF);
        let black = (0x00, 0x00, 0x00);

        assert!((contrast(white, black) - 21.0).abs() < 0.01);
        assert_eq!(contrast(white, black), contrast(black, white));
    }
}
