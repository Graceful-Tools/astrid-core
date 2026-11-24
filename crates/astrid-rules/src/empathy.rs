//! Empathy-state art: which frame a board column shows (ADEV-221, for web's ADEV-142).
//!
//! Ported from `astrid-web/lib/empathy-art/rules.ts` and locked by
//! `contracts/fixtures/empathy-art.json`. Every client must pick the same frame for the same list
//! on the same day, so the rule is one rule: FNV-1a over the list id, plus whole UTC days since
//! the epoch, modulo the metaphors. Pure: "now" is an argument.

use crate::board::ColumnKind;

/// The order matters: the daily rotation indexes into it. Append only.
pub const METAPHORS: [&str; 5] = ["race", "wall", "chef", "garden", "hike"];
/// The states a frame depicts.
pub const STATES: [&str; 5] = ["inbox", "ready", "doing", "waiting", "done"];

const MS_PER_DAY: i64 = 86_400_000;

/// FNV-1a, 32-bit, over the UTF-8 bytes of `text`.
pub fn fnv1a32(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// Whole days since 1970-01-01 in UTC: a new day starts at 00:00 UTC everywhere.
pub fn days_since_epoch_utc(epoch_ms: i64) -> i64 {
    epoch_ms.div_euclid(MS_PER_DAY)
}

/// The list's metaphor for the day: `METAPHORS[(fnv1a32(list_id) + days) % 5]`, summed without
/// 32-bit wraparound. The id is hashed exactly as the server sends it (lowercase UUID text).
pub fn metaphor_for(list_id: &str, epoch_ms: i64) -> &'static str {
    METAPHORS[metaphor_index(METAPHORS.len(), list_id, epoch_ms)]
}

/// The index [`metaphor_for`] picks among `count` metaphors. A client given a manifest uses the
/// array it received rather than a built-in count (new metaphors are appended), so the rotation
/// stays in step with every other client. `count` must not be zero.
pub fn metaphor_index(count: usize, list_id: &str, epoch_ms: i64) -> usize {
    let sum = i64::from(fnv1a32(list_id)) + days_since_epoch_utc(epoch_ms);
    sum.rem_euclid(count as i64) as usize
}

/// A column's state in the wire's terms: the Done column holds completed tasks and every other
/// column is named by the `statusRole` its tasks carry (Inbox's is `None`). A custom column,
/// whatever its role string, gets no frame.
pub fn state_for_column(completed: bool, status_role: Option<&str>) -> Option<&'static str> {
    if completed {
        return Some("done");
    }
    match status_role {
        None => Some("inbox"),
        Some("ready") => Some("ready"),
        Some("doing") => Some("doing"),
        Some("waiting") => Some("waiting"),
        Some(_) => None,
    }
}

/// The state a board column shows, from its id and kind (web's `empathyFrameSrc`): a status
/// column's id is its role, so a custom one maps to none.
pub fn state_for_board_column(id: &str, kind: ColumnKind) -> Option<&'static str> {
    match kind {
        ColumnKind::Inbox => state_for_column(false, None),
        ColumnKind::Done => state_for_column(true, None),
        ColumnKind::Status => state_for_column(false, Some(id)),
    }
}

/// The `(metaphor, state)` of the frame a column shows today, or `None` for a custom column.
pub fn frame_for_column(
    list_id: &str,
    id: &str,
    kind: ColumnKind,
    epoch_ms: i64,
) -> Option<(&'static str, &'static str)> {
    let state = state_for_board_column(id, kind)?;
    Some((metaphor_for(list_id, epoch_ms), state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_starts_at_midnight_utc_and_before_the_epoch_rounds_down() {
        assert_eq!(days_since_epoch_utc(0), 0);
        assert_eq!(days_since_epoch_utc(MS_PER_DAY - 1), 0);
        assert_eq!(days_since_epoch_utc(MS_PER_DAY), 1);
        assert_eq!(days_since_epoch_utc(-1), -1);
    }

    #[test]
    fn a_custom_column_has_no_frame() {
        assert_eq!(state_for_board_column("review", ColumnKind::Status), None);
        assert_eq!(frame_for_column("x", "review", ColumnKind::Status, 0), None);
    }
}
