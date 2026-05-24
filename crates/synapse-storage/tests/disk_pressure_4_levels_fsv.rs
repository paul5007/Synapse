use std::error::Error;

use synapse_core::error_codes;
use synapse_storage::{Db, DiskPressureLevel, cf};

const TEST_SCHEMA_VERSION: u32 = 7;

#[test]
fn disk_pressure_transitions_writes_and_restart_with_fsv() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;

    let transitions = [
        (
            1_500_000_000,
            DiskPressureLevel::Level1,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_1),
        ),
        (1_400_000_000, DiskPressureLevel::Level1, None),
        (
            750_000_000,
            DiskPressureLevel::Level2,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_2),
        ),
        (
            300_000_000,
            DiskPressureLevel::Level3,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_3),
        ),
        (
            100_000_000,
            DiskPressureLevel::Level4,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_4),
        ),
        (
            300_000_000,
            DiskPressureLevel::Level3,
            Some(error_codes::STORAGE_DISK_PRESSURE_LEVEL_3),
        ),
    ];

    for (free_bytes, expected_level, expected_code) in transitions {
        let before = db.pressure_level();
        let report = db.run_pressure_check_with_free_bytes_for_fsv(free_bytes)?;
        let after = db.pressure_level();
        println!(
            "source_of_truth=pressure_state free_bytes={free_bytes} before={before:?} after_truth=level:{after:?},code:{:?},compacted:{} final_value=expected:{expected_level:?}",
            report.emitted_code,
            report.compacted_cfs.len()
        );
        assert_eq!(after, expected_level);
        assert_eq!(report.emitted_code, expected_code);
    }

    db.put_batch(cf::CF_OBSERVATIONS, row("obs-l3"))?;
    db.put_batch(cf::CF_EVENTS, row("event-l3"))?;
    db.flush()?;
    let l3_observations = db.scan_cf(cf::CF_OBSERVATIONS)?;
    let l3_events = db.scan_cf(cf::CF_EVENTS)?;
    println!(
        "source_of_truth=pressure_cf_scan level=Level3 before=empty after_truth=observations:{},events:{} final_value=events:{:?}",
        l3_observations.len(),
        l3_events.len(),
        printable_keys(&l3_events)
    );
    assert!(l3_observations.is_empty());
    assert_eq!(l3_events.len(), 1);

    db.run_pressure_check_with_free_bytes_for_fsv(100_000_000)?;
    db.put_batch(cf::CF_OBSERVATIONS, row("obs-l4"))?;
    db.put_batch(cf::CF_EVENTS, row("event-l4"))?;
    db.put_batch(cf::CF_REFLEX_AUDIT, row("audit-l4"))?;
    db.put_batch(cf::CF_SESSIONS, row("session-l4"))?;
    db.flush()?;
    let l4_observations = db.scan_cf(cf::CF_OBSERVATIONS)?;
    let l4_events = db.scan_cf(cf::CF_EVENTS)?;
    let l4_audit = db.scan_cf(cf::CF_REFLEX_AUDIT)?;
    let l4_sessions = db.scan_cf(cf::CF_SESSIONS)?;
    println!(
        "source_of_truth=pressure_cf_scan level=Level4 before=level3_rows after_truth=observations:{},events:{},audit:{},sessions:{} final_value=audit:{:?},sessions:{:?}",
        l4_observations.len(),
        l4_events.len(),
        l4_audit.len(),
        l4_sessions.len(),
        printable_keys(&l4_audit),
        printable_keys(&l4_sessions)
    );
    assert!(l4_observations.is_empty());
    assert_eq!(l4_events.len(), 1);
    assert_eq!(l4_audit.len(), 1);
    assert_eq!(l4_sessions.len(), 1);
    drop(db);

    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let reopened_audit = reopened.scan_cf(cf::CF_REFLEX_AUDIT)?;
    let reopened_sessions = reopened.scan_cf(cf::CF_SESSIONS)?;
    println!(
        "source_of_truth=pressure_cf_scan edge=restart before=dropped after_truth=audit:{},sessions:{} final_value=durable:{}",
        reopened_audit.len(),
        reopened_sessions.len(),
        reopened_audit.len() == 1 && reopened_sessions.len() == 1
    );
    assert_eq!(reopened_audit.len(), 1);
    assert_eq!(reopened_sessions.len(), 1);
    Ok(())
}

fn row(label: &'static str) -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![(
        label.as_bytes().to_vec(),
        format!(r#"{{"label":"{label}"}}"#).into_bytes(),
    )]
}

fn printable_keys(rows: &[(Vec<u8>, Vec<u8>)]) -> Vec<String> {
    rows.iter()
        .map(|(key, _value)| String::from_utf8_lossy(key).into_owned())
        .collect()
}
