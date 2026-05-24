use std::{error::Error, fs};

use synapse_core::error_codes;
use synapse_storage::{Db, cf};

const TEST_SCHEMA_VERSION: u32 = 7;

#[test]
fn open_all_cfs_and_restart_durability_with_fsv() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    println!(
        "source_of_truth=open_all_cfs before_exists={} after_truth=not_opened final_value=path:{}",
        path.exists(),
        path.display()
    );

    let db = Db::open(&path, TEST_SCHEMA_VERSION)?;
    for cf_name in cf::ALL_COLUMN_FAMILIES {
        let before = db.scan_cf(cf_name)?;
        db.put_batch(cf_name, row(cf_name))?;
        println!(
            "source_of_truth=open_all_cfs cf={cf_name} before={} after_truth=enqueued final_value=key:{}",
            before.len(),
            key(cf_name)
        );
    }
    db.flush()?;
    let after_counts = cf_counts(&db)?;
    println!(
        "source_of_truth=open_all_cfs after_open_counts={after_counts:?} after_truth=all_cfs_writable final_value=count:{}",
        after_counts.len()
    );
    assert!(after_counts.iter().all(|(_cf_name, count)| *count == 1));
    drop(db);

    let reopened = Db::open(&path, TEST_SCHEMA_VERSION)?;
    let reopened_counts = cf_counts(&reopened)?;
    println!(
        "source_of_truth=open_all_cfs after_reopen_counts={reopened_counts:?} after_truth=durable final_value=count:{}",
        reopened_counts.len()
    );
    assert!(reopened_counts.iter().all(|(_cf_name, count)| *count == 1));
    Ok(())
}

#[test]
fn open_rejects_file_path_and_schema_mismatch_with_fsv() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let file_path = temp.path().join("db-file");
    fs::write(&file_path, b"not a directory")?;
    let file_error = match Db::open(&file_path, TEST_SCHEMA_VERSION) {
        Ok(db) => panic!("Db::open accepted a file path: {db:?}"),
        Err(error) => error,
    };
    println!(
        "source_of_truth=open_all_cfs edge=file_path before_is_file=true after_truth=code:{} final_value=still_file:{}",
        file_error.code(),
        file_path.is_file()
    );
    assert_eq!(file_error.code(), error_codes::STORAGE_OPEN_FAILED);

    let schema_path = temp.path().join("schema-db");
    let db = Db::open(&schema_path, 1)?;
    drop(db);
    let schema_error = match Db::open(&schema_path, TEST_SCHEMA_VERSION) {
        Ok(db) => panic!("Db::open accepted a mismatched schema: {db:?}"),
        Err(error) => error,
    };
    println!(
        "source_of_truth=open_all_cfs edge=schema_mismatch before_schema=1 after_truth=code:{} final_value=db_exists:{}",
        schema_error.code(),
        schema_path.exists()
    );
    assert_eq!(schema_error.code(), error_codes::STORAGE_SCHEMA_MISMATCH);
    Ok(())
}

fn cf_counts(db: &Db) -> Result<Vec<(&'static str, usize)>, Box<dyn Error>> {
    cf::ALL_COLUMN_FAMILIES
        .into_iter()
        .map(|cf_name| Ok((cf_name, db.scan_cf(cf_name)?.len())))
        .collect()
}

fn row(cf_name: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![(
        key(cf_name).into_bytes(),
        format!(r#"{{"label":"{cf_name}"}}"#).into_bytes(),
    )]
}

fn key(cf_name: &str) -> String {
    format!("open-all-cfs-{cf_name}")
}
