//! Where a task callout opens from the row it belongs to, and where its arrow points.
//!
//! A multi-column screen — a board, a list beside a detail pane, an iPad in landscape — opens a
//! task's detail as a callout anchored to the row that was tapped. Two things have to be decided
//! before it can be drawn, and both are arithmetic: **which side of the anchor it opens on**, and
//! **how far down its edge the arrow sits** so that the arrow points at the row rather than
//! somewhere near it.
//!
//! Neither is platform-specific, and every client that draws a callout has to answer both. Windows
//! had not answered them at all — its board spliced detail into the column as another card, so
//! there was no side to pick and no arrow to aim (task `011080d4`). Rather than let four clients
//! each invent an answer, the arithmetic is here and the clients lay out what it returns.
//!
//! ## Coordinates
//!
//! One space, the window's, with y increasing downward — which is what every one of these
//! platforms uses for layout. Units are logical pixels; the caller has already divided by its
//! scale factor, because a rule that took a scale factor would be a rule that could get it wrong.
//!
//! ## What this does not decide
//!
//! The callout's SIZE. How big a detail pane wants to be is a design decision per platform and per
//! density, and a rule that returned one would be choosing the layout rather than placing it. The
//! caller measures, then asks.

/// A rectangle in the window's coordinates, y increasing downward.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(left: f64, top: f64, width: f64, height: f64) -> Rect {
        Rect {
            left,
            top,
            width,
            height,
        }
    }

    pub fn right(&self) -> f64 {
        self.left + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.top + self.height
    }

    fn middle(&self) -> f64 {
        self.top + (self.height / 2.0)
    }
}

/// Which side of the anchor the callout opened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Left,
    Right,
}

/// Where to put the callout, and where to draw its arrow.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub side: Side,
    /// The callout's own box, already clamped into the viewport.
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    /// How far DOWN the callout's anchored edge to draw the arrow, from `top`. Clamped away from
    /// the corners, so it is never drawn over a rounded one.
    pub arrow_offset: f64,
}

/// The space kept between the callout and the viewport's edges: a callout that hugs the window
/// edge reads as clipped.
///
/// Not between the callout and its ANCHOR. The callout touches the row it belongs to — its near
/// edge, where the arrow's tip is, sits on the row's edge (astrid-ios AITD-679, Jon: "the arrow
/// doesn't touch the task row... It should"). It used to keep this gutter there too, and clients
/// that docked the panel to the window edge instead left it nowhere near the row.
pub const GUTTER: f64 = 8.0;

/// How close to a corner the arrow may be drawn.
///
/// The arrow is a triangle on the callout's edge, and a rounded corner eats the edge it would
/// stand on — so an arrow inside this margin is drawn over the curve and reads as a notch rather
/// than a point. Larger than the corner radii these clients use (12 at most), by enough that the
/// triangle's own base clears the curve.
pub const ARROW_MARGIN: f64 = 20.0;

/// Place a callout against `anchor`, inside `viewport`, at the size the caller measured.
///
/// The side is **whichever has more room** — not a preferred side with a fallback, because on a
/// board the column that was tapped is as likely to be at the right edge as the left.
///
/// Everything is then clamped into the viewport: a callout allowed to hang off the window is one
/// the reader cannot finish reading, which is worse than one overlapping the column it came from.
pub fn place(anchor: Rect, viewport: Rect, width: f64, height: f64) -> Placement {
    let room_right = viewport.right() - anchor.right();
    let room_left = anchor.left - viewport.left;
    let side = if room_right >= room_left {
        Side::Right
    } else {
        Side::Left
    };

    // The near edge on the anchor's edge: the arrow's tip touches the row.
    let wanted_left = match side {
        Side::Right => anchor.right(),
        Side::Left => anchor.left - width,
    };
    let left = clamp(
        wanted_left,
        viewport.left + GUTTER,
        viewport.right() - GUTTER - width,
    );

    // Centred on the anchor, then clamped — a row near either edge must not push the callout off.
    let top = clamp(
        anchor.middle() - (height / 2.0),
        viewport.top + GUTTER,
        viewport.bottom() - GUTTER - height,
    );

    // The arrow points at the ROW's middle wherever the callout ended up, which is the whole
    // point: after clamping, the callout's own middle is usually not the row's.
    let arrow_offset = clamp(
        anchor.middle() - top,
        ARROW_MARGIN,
        (height - ARROW_MARGIN).max(ARROW_MARGIN),
    );

    Placement {
        side,
        left,
        top,
        width,
        height,
        arrow_offset,
    }
}

/// `f64::clamp` panics when the bounds cross, which they do whenever the callout is bigger than
/// the viewport it has to fit in — a small window, or a tablet in split view. The low bound wins
/// there, so the callout is pinned to the top-left and overflows the far edge rather than the near
/// one, and the reader can still see where it starts.
fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if high < low {
        return low;
    }
    value.clamp(low, high)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1400x900 window, a column of 320 starting at `column_left`, and a row 64 tall in it.
    fn anchor(column_left: f64, row_top: f64) -> Rect {
        Rect::new(column_left, row_top, 320.0, 64.0)
    }

    fn window() -> Rect {
        Rect::new(0.0, 0.0, 1400.0, 900.0)
    }

    /// The side is whichever has MORE room, which is the whole of the description's first ask.
    #[test]
    fn a_callout_opens_on_the_side_with_more_room_011080d4() {
        // A column at the left: everything is to the right of it.
        let left_column = place(anchor(40.0, 300.0), window(), 420.0, 520.0);
        assert_eq!(left_column.side, Side::Right);
        assert_eq!(left_column.left, 40.0 + 320.0);

        // A column at the right: the room is on the left, so it opens leftward and its own right
        // edge sits on the anchor's left edge (AITD-679).
        let right_column = place(anchor(1000.0, 300.0), window(), 420.0, 520.0);
        assert_eq!(right_column.side, Side::Left);
        assert_eq!(right_column.left + 420.0, 1000.0);
    }

    /// The arrow points at the ROW, not at the callout's own middle — they are only the same when
    /// nothing had to be clamped.
    #[test]
    fn the_arrow_points_at_the_rows_middle_011080d4() {
        let row = anchor(40.0, 300.0);
        let placed = place(row, window(), 420.0, 520.0);

        // Nothing clamped here, so the callout is centred and the arrow is at its half-height.
        assert_eq!(placed.arrow_offset, 260.0);
        assert_eq!(placed.top + placed.arrow_offset, row.top + 32.0);
    }

    /// A row near the top: the callout is pushed down into the window, and the arrow has to travel
    /// UP its edge to keep pointing at the row. This is the case a client gets wrong by drawing
    /// the arrow at the callout's midpoint.
    #[test]
    fn a_row_near_an_edge_moves_the_arrow_not_the_point_011080d4() {
        let row = anchor(40.0, 10.0);
        let placed = place(row, window(), 420.0, 520.0);

        assert_eq!(placed.top, GUTTER, "clamped to the top of the window");
        assert_eq!(
            placed.top + placed.arrow_offset,
            row.top + 32.0,
            "and still points at the row"
        );
        assert!(placed.arrow_offset < 260.0, "so the arrow rose up the edge");

        // And the same at the bottom. A row fully on screen: 800..864, middle 832.
        let low = anchor(40.0, 800.0);
        let placed = place(low, window(), 420.0, 520.0);
        assert_eq!(placed.bottom_of(), 900.0 - GUTTER, "clamped to the bottom");
        assert_eq!(
            placed.top + placed.arrow_offset,
            low.top + 32.0,
            "and still points at the row"
        );
        assert!(
            placed.arrow_offset > 260.0,
            "so the arrow fell down the edge"
        );
    }

    /// The arrow is kept off the corners. A row far enough outside the callout's run would
    /// otherwise put the arrow on the rounded corner, where it draws as a notch.
    #[test]
    fn the_arrow_never_reaches_a_corner_011080d4() {
        // A COMPACT row — 24 tall, which a dense list draws — at the very top of the window. Its
        // middle is 12, the callout is clamped to a gutter from the top, so the point the arrow
        // wants is 4 down the edge: inside the corner. A row of ordinary height never reaches
        // this, which is why the case is written with a short one rather than invented numbers.
        let compact = Rect::new(40.0, 0.0, 320.0, 24.0);
        let placed = place(compact, window(), 420.0, 120.0);
        assert_eq!(placed.top, GUTTER);
        assert_eq!(placed.arrow_offset, ARROW_MARGIN);

        // And the far corner, where a short callout beside a low row would otherwise point below
        // its own bottom edge.
        let low = place(anchor(40.0, 880.0), window(), 420.0, 120.0);
        assert_eq!(low.arrow_offset, 120.0 - ARROW_MARGIN);
    }

    /// A callout larger than the window it must fit in — a small window, or a tablet in split
    /// view. `f64::clamp` panics when the bounds cross; this pins to the near edge instead.
    #[test]
    fn a_callout_bigger_than_the_window_is_pinned_not_a_panic_011080d4() {
        let tiny = Rect::new(0.0, 0.0, 400.0, 300.0);
        let placed = place(anchor(10.0, 10.0), tiny, 420.0, 520.0);
        assert_eq!(placed.left, GUTTER);
        assert_eq!(placed.top, GUTTER);
    }

    /// The callout TOUCHES its anchor (astrid-ios AITD-679, Jon: "the arrow doesn't touch the
    /// task row... It should"). Its near edge, where the arrow's tip is, sits on the row's edge on
    /// either side. The gutter is only the clearance from the viewport.
    #[test]
    fn the_callout_touches_its_anchor_aitd_679() {
        let right = place(anchor(40.0, 300.0), window(), 420.0, 520.0);
        assert_eq!(
            right.left,
            40.0 + 320.0,
            "the arrow's tip on the row's right edge"
        );

        let left = place(anchor(1000.0, 300.0), window(), 420.0, 520.0);
        assert_eq!(
            left.left + left.width,
            1000.0,
            "the arrow's tip on the row's left edge"
        );
    }

    /// Ties go right. Stated so it is a decision rather than an accident of `>=`.
    #[test]
    fn equal_room_opens_right_011080d4() {
        // 1400 wide, a 320 column centred: 540 each side.
        let placed = place(anchor(540.0, 300.0), window(), 420.0, 520.0);
        assert_eq!(placed.side, Side::Right);
    }

    impl Placement {
        fn bottom_of(&self) -> f64 {
            self.top + self.height
        }
    }
}
