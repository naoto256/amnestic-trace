//! Read-only projection of stored snapshots and live delivery state.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;

use serde::Serialize;

use crate::recall::epoch_seconds;
use crate::store::{Row, Store};

#[derive(Serialize)]
pub struct Record {
    session_id: String,
    snapshot: Option<Row>,
    delivery: Delivery,
}

#[derive(Default, Serialize)]
struct Delivery {
    marker: Option<String>,
    deadline_epoch_seconds: Option<u64>,
    deadline_remaining_seconds: Option<i64>,
    claims: Vec<Claim>,
}

#[derive(Serialize)]
struct Claim {
    file: String,
    operation: String,
    state: Option<String>,
}

/// Returns every matching snapshot, including orphan operational state that
/// has no row. Filters are combined with AND and observation creates nothing.
pub fn records(session: Option<&str>, key: Option<&str>) -> io::Result<Vec<Record>> {
    let Some(store) = Store::open_existing()? else {
        return Ok(Vec::new());
    };
    records_from(&store, session, key)
}

/// Renders the diagnostic projection for its intended audience.
///
/// The default is deliberately a labeled, line-oriented view for a person at
/// a terminal. JSON is opt-in so scripts get a stable machine representation
/// without making the interactive command expose implementation serialization
/// as its user interface.
pub fn render(records: &[Record], json: bool) -> io::Result<String> {
    if json {
        return serde_json::to_string(records).map_err(io::Error::other);
    }
    if records.is_empty() {
        return Ok("No matching snapshots or delivery state.".to_string());
    }

    let mut output = String::new();
    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        writeln!(output, "Session: {}", record.session_id)
            .expect("writing to a String cannot fail");
        match &record.snapshot {
            Some(snapshot) => {
                writeln!(output, "  Snapshot:").expect("writing to a String cannot fail");
                writeln!(output, "    Session ID: {}", snapshot.session_id)
                    .expect("writing to a String cannot fail");
                writeln!(
                    output,
                    "    AMTR key: {}",
                    snapshot.amtr_key.as_deref().unwrap_or("<none>")
                )
                .expect("writing to a String cannot fail");
                writeln!(output, "    Compacted at: {}", snapshot.compacted_at)
                    .expect("writing to a String cannot fail");
                writeln!(output, "    Handoff:").expect("writing to a String cannot fail");
                write_block(&mut output, "      ", &snapshot.handoff);
            }
            None => {
                writeln!(output, "  Snapshot: <none>").expect("writing to a String cannot fail")
            }
        }
        writeln!(output, "  Delivery:").expect("writing to a String cannot fail");
        writeln!(
            output,
            "    Marker: {}",
            record.delivery.marker.as_deref().unwrap_or("<none>")
        )
        .expect("writing to a String cannot fail");
        writeln!(
            output,
            "    Deadline epoch seconds: {}",
            optional_number(record.delivery.deadline_epoch_seconds)
        )
        .expect("writing to a String cannot fail");
        writeln!(
            output,
            "    Deadline remaining seconds: {}",
            optional_number(record.delivery.deadline_remaining_seconds)
        )
        .expect("writing to a String cannot fail");
        if record.delivery.claims.is_empty() {
            writeln!(output, "    Claims: <none>").expect("writing to a String cannot fail");
        } else {
            writeln!(output, "    Claims:").expect("writing to a String cannot fail");
            for claim in &record.delivery.claims {
                writeln!(output, "      - File: {}", claim.file)
                    .expect("writing to a String cannot fail");
                writeln!(output, "        Operation: {}", claim.operation)
                    .expect("writing to a String cannot fail");
                match &claim.state {
                    Some(state) => {
                        writeln!(output, "        State:")
                            .expect("writing to a String cannot fail");
                        write_block(&mut output, "          ", state);
                    }
                    None => writeln!(output, "        State: <unreadable>")
                        .expect("writing to a String cannot fail"),
                }
            }
        }
    }
    while output.ends_with('\n') {
        output.pop();
    }
    Ok(output)
}

fn write_block(output: &mut String, indent: &str, value: &str) {
    for line in value.split('\n') {
        writeln!(output, "{indent}{line}").expect("writing to a String cannot fail");
    }
}

fn optional_number<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(|| "<none>".to_string(), |number| number.to_string())
}

/// Read-only projection.
///
/// Building from `rows` first and then folding in operational files means an
/// orphan marker or a stray claim (a worker that crashed mid-extraction, a
/// deadline whose row never landed) still shows up in the output. `peek` is
/// the diagnostic tool used when something looks stuck, so surfacing the
/// stuck state matters more than presenting a clean snapshot-oriented view.
/// `BTreeMap` sorts by session_id for stable output across runs; filters are
/// applied last so an empty filter doesn't cost the deterministic order.
fn records_from(
    store: &Store,
    session: Option<&str>,
    key: Option<&str>,
) -> io::Result<Vec<Record>> {
    let mut records: BTreeMap<String, Record> = store
        .rows()?
        .into_iter()
        .map(|row| {
            (
                row.session_id.clone(),
                Record {
                    session_id: row.session_id.clone(),
                    snapshot: Some(row),
                    delivery: Delivery::default(),
                },
            )
        })
        .collect();

    let entries = match fs::read_dir(store.cortex()) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((session_id, kind)) = classify(&name) else {
            continue;
        };
        let record = records
            .entry(session_id.to_string())
            .or_insert_with(|| Record {
                session_id: session_id.to_string(),
                snapshot: None,
                delivery: Delivery::default(),
            });
        let value = fs::read_to_string(entry.path()).ok();
        match kind {
            Kind::Marker => record.delivery.marker = value,
            Kind::Deadline => {
                let deadline = value.as_deref().and_then(|raw| raw.parse::<u64>().ok());
                record.delivery.deadline_epoch_seconds = deadline;
                record.delivery.deadline_remaining_seconds = deadline.map(|at| {
                    let now = epoch_seconds().unwrap_or(0);
                    (i128::from(at) - i128::from(now)).clamp(i64::MIN.into(), i64::MAX.into())
                        as i64
                });
            }
            Kind::Claim(operation) => record.delivery.claims.push(Claim {
                file: name.clone(),
                operation: operation.to_string(),
                state: value,
            }),
        }
    }

    let mut result: Vec<_> = records
        .into_values()
        .filter(|record| session.is_none_or(|wanted| record.session_id == wanted))
        .filter(|record| {
            key.is_none_or(|wanted| {
                record
                    .snapshot
                    .as_ref()
                    .and_then(|row| row.amtr_key.as_deref())
                    == Some(wanted)
            })
        })
        .collect();
    for record in &mut result {
        record.delivery.claims.sort_by(|a, b| a.file.cmp(&b.file));
    }
    Ok(result)
}

enum Kind<'a> {
    Marker,
    Deadline,
    Claim(&'a str),
}

/// Classifies an on-disk filename back into (session_id, kind of operational
/// state). Row files (`<session>.json`) are matched upstream and never reach
/// here — they enter `records_from` through `store.rows()`, not the directory
/// scan. Anything unrecognized is `None` (the temp files `write_atomic` uses,
/// the user's `prompt.md`, unknown crumbs from earlier versions) rather than
/// misclassified as an unknown claim shape.
///
/// The two `strip_suffix` arms exclude claim files by construction — a claim
/// name has more suffix past `.marker` / `.deliver-deadline` — so the arm
/// order is defensive rather than load-bearing.
fn classify(name: &str) -> Option<(&str, Kind<'_>)> {
    if let Some(session) = name.strip_suffix(".marker") {
        Some((session, Kind::Marker))
    } else if let Some(session) = name.strip_suffix(".deliver-deadline") {
        Some((session, Kind::Deadline))
    } else if let Some((session, _)) = name.split_once(".marker.delivering.") {
        Some((session, Kind::Claim("delivering")))
    } else if let Some((session, _)) = name.split_once(".marker.expiring.") {
        Some((session, Kind::Claim("expiring")))
    } else if let Some((session, _)) = name.split_once(".deliver-deadline.publishing.") {
        Some((session, Kind::Claim("publishing")))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> Store {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("amtr-peek-test-{}-{sequence}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        Store::at(base).unwrap()
    }

    #[test]
    fn classifies_every_operational_file_without_treating_rows_as_claims() {
        assert!(matches!(classify("s.marker"), Some(("s", Kind::Marker))));
        assert!(matches!(
            classify("s.deliver-deadline"),
            Some(("s", Kind::Deadline))
        ));
        assert!(matches!(
            classify("s.marker.delivering.1.2"),
            Some(("s", Kind::Claim("delivering")))
        ));
        assert!(classify("s.json").is_none());
    }

    #[test]
    fn observation_includes_orphan_delivery_state() {
        let store = scratch();
        fs::write(store.marker_path("orphan"), "ongoing").unwrap();
        let records = records_from(&store, None, None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "orphan");
        assert!(records[0].snapshot.is_none());
        assert_eq!(records[0].delivery.marker.as_deref(), Some("ongoing"));
    }

    #[test]
    fn a_missing_cortex_is_an_empty_read_only_store() {
        let store = scratch();
        fs::remove_dir_all(store.cortex()).unwrap();

        assert!(records_from(&store, None, None).unwrap().is_empty());
    }

    #[test]
    fn deadline_remainder_clamps_before_narrowing() {
        let store = scratch();
        store
            .save(&Row {
                session_id: "s".into(),
                amtr_key: None,
                handoff: "state".into(),
                compacted_at: "2026-08-09T00:00:00.000Z".into(),
            })
            .unwrap();
        fs::write(store.deadline_path("s"), u64::MAX.to_string()).unwrap();

        let records = records_from(&store, None, None).unwrap();
        assert_eq!(
            records[0].delivery.deadline_remaining_seconds,
            Some(i64::MAX)
        );
    }

    #[test]
    fn filters_combine_and_full_snapshot_metadata_is_preserved() {
        let store = scratch();
        let row = Row {
            session_id: "wanted".into(),
            amtr_key: Some("amtr-k".into()),
            handoff: "## Working state\nall metadata".into(),
            compacted_at: "2026-08-09T00:00:00.000Z".into(),
        };
        store.save(&row).unwrap();
        store
            .save(&Row {
                session_id: "other".into(),
                amtr_key: Some("amtr-other".into()),
                ..row.clone()
            })
            .unwrap();
        store.mark_ready("wanted", "amtr-k").unwrap();
        fs::write(
            store.deadline_path("wanted"),
            (epoch_seconds().unwrap() + 5).to_string(),
        )
        .unwrap();
        fs::write(
            store.cortex().join("wanted.marker.delivering.1.2"),
            "ready:amtr-k",
        )
        .unwrap();

        let records = records_from(&store, Some("wanted"), Some("amtr-k")).unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.snapshot.as_ref(), Some(&row));
        assert_eq!(record.delivery.marker.as_deref(), Some("ready:amtr-k"));
        assert!(record.delivery.deadline_remaining_seconds.is_some());
        assert_eq!(record.delivery.claims.len(), 1);

        assert!(
            records_from(&store, Some("wanted"), Some("amtr-other"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn default_render_is_human_readable_and_json_is_opt_in() {
        let records = vec![Record {
            session_id: "session-a".into(),
            snapshot: Some(Row {
                session_id: "session-a".into(),
                amtr_key: Some("amtr-a".into()),
                handoff: "first line\nsecond line".into(),
                compacted_at: "2026-08-09T00:00:00.000Z".into(),
            }),
            delivery: Delivery {
                marker: Some("ready:amtr-a".into()),
                deadline_epoch_seconds: Some(42),
                deadline_remaining_seconds: Some(7),
                claims: vec![Claim {
                    file: "session-a.marker.delivering.1.2".into(),
                    operation: "delivering".into(),
                    state: Some("ready:amtr-a".into()),
                }],
            },
        }];

        let human = render(&records, false).unwrap();
        assert!(human.starts_with("Session: session-a\n  Snapshot:\n"));
        assert!(human.contains("    Session ID: session-a\n"));
        assert!(human.contains("    AMTR key: amtr-a\n"));
        assert!(human.contains("      first line\n      second line\n"));
        assert!(human.contains("    Marker: ready:amtr-a\n"));
        assert!(human.contains("      - File: session-a.marker.delivering.1.2\n"));
        assert!(!human.starts_with('['));

        let json = render(&records, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["session_id"], "session-a");
        assert_eq!(parsed[0]["snapshot"]["amtr_key"], "amtr-a");
        assert_eq!(
            parsed[0]["delivery"]["claims"][0]["operation"],
            "delivering"
        );
    }

    #[test]
    fn empty_human_projection_is_explicit() {
        assert_eq!(
            render(&[], false).unwrap(),
            "No matching snapshots or delivery state."
        );
        assert_eq!(render(&[], true).unwrap(), "[]");
    }
}
