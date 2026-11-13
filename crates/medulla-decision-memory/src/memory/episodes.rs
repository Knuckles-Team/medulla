//! Episode validity and time-visible recall (MEDULLA-DECISION-ENGINE-R155.2).
//!
//! Pure functions over a sealed [`Episode`] and the `(record id, timestamp_ms)`
//! stamps of the records it holds. Nothing here reads a clock or a store; the
//! caller supplies `as_of_ms`.

use medulla_work_types::memory::Episode;

/// The time stamp of one memory record, as the episode rules read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordStamp {
    /// The memory record's id.
    pub record_id: String,
    /// Milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
}

/// Why an episode is not valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeFault {
    /// The stamps are not the episode's records, in the episode's order.
    RecordSetMismatch,
    /// A record's timestamp is lower than the one before it.
    RecordsOutOfOrder,
    /// A record's timestamp lies outside `[start_ms, end_ms]`.
    RecordOutsideWindow,
}

/// An episode is valid when its records, in `record_ids` order, have
/// non-decreasing `timestamp_ms` inside `[start_ms, end_ms]`.
pub fn valid_episode(episode: &Episode, records: &[RecordStamp]) -> Result<(), EpisodeFault> {
    let same_set = records.len() == episode.record_ids.len()
        && records
            .iter()
            .zip(episode.record_ids.iter())
            .all(|(stamp, id)| stamp.record_id == id.as_str());
    if !same_set {
        return Err(EpisodeFault::RecordSetMismatch);
    }
    let mut previous = i64::MIN;
    for stamp in records {
        if stamp.timestamp_ms < previous {
            return Err(EpisodeFault::RecordsOutOfOrder);
        }
        if stamp.timestamp_ms < episode.start_ms || stamp.timestamp_ms > episode.end_ms {
            return Err(EpisodeFault::RecordOutsideWindow);
        }
        previous = stamp.timestamp_ms;
    }
    Ok(())
}

/// Two windows of one session overlap when they share more than an endpoint,
/// or are the same window.
fn windows_overlap(a: &Episode, b: &Episode) -> bool {
    (a.start_ms < b.end_ms && b.start_ms < a.end_ms)
        || (a.start_ms == b.start_ms && a.end_ms == b.end_ms)
}

/// The first pair of episodes of one session whose windows overlap, in the
/// order given, or `None` when every session is clean. Cost is O(n log n) in
/// the episodes in scope.
pub fn first_session_overlap(episodes: &[Episode]) -> Option<(&Episode, &Episode)> {
    let mut order: Vec<&Episode> = episodes.iter().collect();
    order.sort_by(|a, b| {
        (a.session_id.as_str(), a.start_ms, a.episode_id.as_str()).cmp(&(
            b.session_id.as_str(),
            b.start_ms,
            b.episode_id.as_str(),
        ))
    });
    // Within one session sorted by start, an overlap with any later episode
    // implies an overlap with the next one or with the running maximum end.
    let mut run: Option<&Episode> = None;
    for episode in order {
        if let Some(open) = run {
            if open.session_id == episode.session_id {
                if windows_overlap(open, episode) {
                    return Some((open, episode));
                }
                if episode.end_ms > open.end_ms {
                    run = Some(episode);
                }
                continue;
            }
        }
        run = Some(episode);
    }
    None
}

/// The episodes of `tenant` that were sealed by `as_of_ms` (their `end_ms` at
/// or below it), sorted by `start_ms`, then `episode_id`. Cost is
/// O(episodes in scope).
pub fn episodes_visible_at<'a>(
    episodes: &'a [Episode],
    tenant: &str,
    as_of_ms: i64,
) -> Vec<&'a Episode> {
    let mut visible: Vec<&Episode> = episodes
        .iter()
        .filter(|e| e.tenant.as_str() == tenant && e.end_ms <= as_of_ms)
        .collect();
    visible.sort_by(|a, b| {
        (a.start_ms, a.episode_id.as_str()).cmp(&(b.start_ms, b.episode_id.as_str()))
    });
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(
        id: &str,
        tenant: &str,
        session: &str,
        start: i64,
        end: i64,
        ids: &[&str],
    ) -> Episode {
        Episode::seal(
            id,
            tenant,
            session,
            start,
            end,
            ids.iter().map(|s| s.to_string()).collect(),
        )
        .unwrap()
    }

    fn stamps(items: &[(&str, i64)]) -> Vec<RecordStamp> {
        items
            .iter()
            .map(|(id, ts)| RecordStamp {
                record_id: id.to_string(),
                timestamp_ms: *ts,
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R155.2
    #[test]
    fn episodes_sort_by_time_and_hide_future() {
        let all = vec![
            episode("e:b", "t1", "s1", 100, 200, &["r1"]),
            episode("e:a", "t1", "s2", 100, 150, &["r2"]),
            episode("e:c", "t1", "s1", 300, 400, &["r3"]),
            episode("e:other", "t2", "s9", 50, 60, &["r4"]),
        ];
        let seen = episodes_visible_at(&all, "t1", 250);
        let ids: Vec<&str> = seen.iter().map(|e| e.episode_id.as_str()).collect();
        // Same start sorts by id; the future episode and the other tenant are hidden.
        assert_eq!(ids, vec!["e:a", "e:b"]);
        // An episode ending exactly at as_of is visible.
        assert_eq!(episodes_visible_at(&all, "t1", 400).len(), 3);
        assert!(episodes_visible_at(&all, "t1", 199)
            .iter()
            .all(|e| e.end_ms <= 199));
        // Records inside the window, non-decreasing, make a valid episode.
        let good = episode("e:v", "t1", "s1", 100, 200, &["r1", "r2", "r3"]);
        assert_eq!(
            valid_episode(&good, &stamps(&[("r1", 100), ("r2", 100), ("r3", 200)])),
            Ok(())
        );
        assert_eq!(
            valid_episode(&good, &stamps(&[("r1", 150), ("r2", 120), ("r3", 160)])),
            Err(EpisodeFault::RecordsOutOfOrder)
        );
        assert_eq!(
            valid_episode(&good, &stamps(&[("r1", 90), ("r2", 120), ("r3", 160)])),
            Err(EpisodeFault::RecordOutsideWindow)
        );
        assert_eq!(
            valid_episode(&good, &stamps(&[("r1", 100), ("r2", 120)])),
            Err(EpisodeFault::RecordSetMismatch)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R155.2
    #[test]
    fn overlapping_episodes_are_invalid() {
        let a = episode("e:1", "t1", "s1", 100, 200, &["r1"]);
        let touching = episode("e:2", "t1", "s1", 200, 300, &["r2"]);
        let overlapping = episode("e:3", "t1", "s1", 150, 250, &["r3"]);
        let other_session = episode("e:4", "t1", "s2", 150, 250, &["r4"]);
        let same_window = episode("e:5", "t1", "s1", 100, 200, &["r5"]);
        // Sharing only an endpoint, or overlapping across sessions, is allowed.
        assert!(first_session_overlap(&[a.clone(), touching.clone(), other_session]).is_none());
        let found = first_session_overlap(&[touching.clone(), a.clone(), overlapping]).unwrap();
        assert_eq!(found.0.session_id, found.1.session_id);
        assert!(first_session_overlap(&[a, same_window]).is_some());
    }
}
