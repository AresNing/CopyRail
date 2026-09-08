//! Production storage API, synthetic on-disk history only. No native services.
//! Run optimized: cargo run --release -p paste-storage --example history_benchmark -- seed
//! Re-measure exactly that dataset: ... -- measure <printed directory>
use chrono::{Duration, Utc};
use paste_domain::{
    CaptureFlags, CapturedItem, CapturedRepresentation, ContentKind, DeviceId, DeviceMetadata,
    SearchFilters, SearchPage, SearchQuery, SourceApplication,
};
use paste_storage::SqliteStore;
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Instant,
};

const ROWS: usize = 100_000;
const REPEATS: usize = 40;

fn main() -> Result<(), Box<dyn Error>> {
    if cfg!(debug_assertions) {
        return Err("benchmark requires --release".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/history-performance");
    fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let [mode, directory] = args.as_slice()
        && mode == "probe"
    {
        return probe(&root, Path::new(directory));
    }
    if let [mode, directory] = args.as_slice()
        && mode == "probe-compact"
    {
        return probe_compact(&root, Path::new(directory));
    }
    let directory = match args.as_slice() {
        [mode] if mode == "seed" => seed(&root)?,
        [mode, directory] if mode == "measure" => {
            let directory = PathBuf::from(directory).canonicalize()?;
            if directory.parent() != Some(root.as_path()) {
                return Err("only this benchmark's direct artifact directory is allowed".into());
            }
            directory
        }
        [mode, directory] if mode == "upgrade-copy" => {
            let source = validate_synthetic(&root, Path::new(directory))?;
            let target = tempfile::Builder::new().prefix("synthetic-upgrade-").tempdir_in(&root)?.keep();
            copy_database(&source.join("history.sqlite3"), &target.join("history.sqlite3"))?;
            fs::copy(source.join("synthetic-fixture.json"), target.join("synthetic-fixture.json"))?;
            println!("UPGRADE COPY {} -> {}", source.display(), target.display());
            target
        }
        _ => return Err("usage: history_benchmark seed | measure <synthetic directory> | upgrade-copy <synthetic directory>".into()),
    };
    let metadata: Value =
        serde_json::from_slice(&fs::read(directory.join("synthetic-fixture.json"))?)?;
    if metadata["kind"] != "pasters-history-benchmark-v1" || metadata["rows"] != ROWS {
        return Err("not a completed synthetic benchmark dataset".into());
    }
    let store = SqliteStore::open(directory.join("history.sqlite3"))?;
    let first_device =
        DeviceId::from_str(metadata["firstDevice"].as_str().ok_or("device metadata")?)?;
    let board = store
        .list_pinboards()?
        .into_iter()
        .find(|p| p.name == "Synthetic Benchmark")
        .ok_or("synthetic board missing")?;
    let page = SearchPage::new(200, 0); // Same page size as the actual UI.
    let mut cases = Vec::new();
    for (name, text, filters, expected) in [
        ("unique_term", "record099999", SearchFilters::default(), 1),
        ("medium_term", "bucket042", SearchFilters::default(), 200),
        ("common_term", "clipboard", SearchFilters::default(), 200),
        (
            "common_two_terms",
            "Rust clipboard",
            SearchFilters::default(),
            200,
        ),
        ("cjk_term", "项目笔记", SearchFilters::default(), 200),
        (
            "missing_term",
            "notpresentanywhere",
            SearchFilters::default(),
            0,
        ),
        (
            "type_only",
            "",
            SearchFilters {
                content_kinds: vec![ContentKind::Text],
                ..Default::default()
            },
            200,
        ),
        (
            "source_only",
            "",
            SearchFilters {
                source_bundle_ids: vec!["io.pasters.synthetic.3".into()],
                ..Default::default()
            },
            200,
        ),
        (
            "source_with_common_term",
            "clipboard",
            SearchFilters {
                source_bundle_ids: vec!["io.pasters.synthetic.3".into()],
                ..Default::default()
            },
            200,
        ),
        (
            "device_with_common_term",
            "clipboard",
            SearchFilters {
                device_ids: vec![first_device],
                ..Default::default()
            },
            200,
        ),
        (
            "board_only",
            "",
            SearchFilters {
                pinboard_ids: vec![board.id],
                ..Default::default()
            },
            200,
        ),
        (
            "board_with_common_term",
            "clipboard",
            SearchFilters {
                pinboard_ids: vec![board.id],
                ..Default::default()
            },
            200,
        ),
    ] {
        let query = SearchQuery {
            text: text.into(),
            filters,
        };
        let result = timed(name, || {
            let hits = store.search_items(&query, page)?;
            assert_eq!(hits.len(), expected, "{name}");
            assert!(hits.iter().all(|hit| {
                hit.source
                    .bundle_identifier
                    .starts_with("io.pasters.synthetic.")
            }));
            std::hint::black_box(hits);
            Ok(())
        })?;
        // Untimed parity: omitting unused BM25 must not change any returned
        // item, metadata, ordering, filter or pagination behavior.
        let ranked = store
            .search(&query, page)?
            .into_iter()
            .map(|hit| hit.item)
            .collect::<Vec<_>>();
        assert_eq!(
            store.search_items(&query, page)?,
            ranked,
            "ranked/items parity: {name}"
        );
        println!("{result}");
        cases.push(result);
    }
    for (name, offset) in [("history_first_page", 0), ("history_deep_page", 90_000)] {
        let result = timed(name, || {
            let items = store.list_history(SearchPage::new(200, offset))?;
            assert_eq!(items.len(), 200);
            std::hint::black_box(items);
            Ok(())
        })?;
        println!("{result}");
        cases.push(result);
    }
    let facets = timed("search_facets", || {
        let facets = store.list_search_facets()?;
        assert_eq!(facets.sources.len(), 8);
        assert_eq!(facets.devices.len(), 3);
        assert_eq!(
            facets
                .sources
                .iter()
                .map(|s| s.item_count as usize)
                .sum::<usize>(),
            ROWS
        );
        std::hint::black_box(facets);
        Ok(())
    })?;
    println!("{facets}");
    cases.push(facets);
    let elapsed_stamp = Utc::now().timestamp_millis();
    let report = json!({
        "kind":"pasters-history-benchmark-v1", "scope":"production storage API only; not native/UI latency or app RSS", "profile":"release", "architecture":std::env::consts::ARCH,
        "rows":ROWS, "pageSize":200, "warmSamplesPerCase":REPEATS,
        "searchApi":"SqliteStore::search_items (timeline and MCP)",
        "migrationBlake3":blake3::hash(include_bytes!("../migrations/0019_search_documents.sql")).to_hex().to_string(),
        "dataset":directory, "sourceBlake3":blake3::hash(include_bytes!("../src/lib.rs")).to_hex().to_string(),
        "fixture":metadata, "cases":cases,
    });
    let report_path = directory.join(format!("report-{elapsed_stamp}.json"));
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    println!("REPORT {}", report_path.display());
    Ok(())
}

fn timed(
    name: &str,
    mut run: impl FnMut() -> Result<(), Box<dyn Error>>,
) -> Result<Value, Box<dyn Error>> {
    let first = Instant::now();
    run()?;
    let first_ms = first.elapsed().as_secs_f64() * 1000.0;
    let mut times = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let start = Instant::now();
        run()?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let percentile =
        |percent: usize| times[(times.len() * percent).div_ceil(100).saturating_sub(1)];
    Ok(
        json!({"name":name, "firstMs":first_ms, "p50Ms":percentile(50), "p95Ms":percentile(95), "maxMs":times[times.len()-1], "samples":REPEATS}),
    )
}

fn seed(root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let directory = tempfile::Builder::new()
        .prefix("synthetic-")
        .tempdir_in(root)?
        .keep();
    println!("DATASET {}", directory.display());
    let store = SqliteStore::open(directory.join("history.sqlite3"))?;
    // get_or_create_device is the persistent *local* identity, not a factory
    // for peers. Use distinct synthetic peer IDs in captured metadata.
    let devices = (0..3)
        .map(|i| DeviceMetadata {
            id: DeviceId::new(),
            display_name: format!("Synthetic {i}"),
        })
        .collect::<Vec<_>>();
    let base = Utc::now() - Duration::seconds(ROWS as i64);
    let mut pins = Vec::new();
    let started = Instant::now();
    for batch in 0..ROWS / 250 {
        let captures = (batch * 250..(batch + 1) * 250).map(|i| CapturedItem {
            captured_at: base + Duration::seconds(i as i64),
            source: SourceApplication { bundle_identifier: format!("io.pasters.synthetic.{}", i % 8), display_name: format!("Synthetic {}", i % 8) },
            device: devices[i % 3].clone(), flags: CaptureFlags::default(),
            representations: vec![CapturedRepresentation::plain_text(format!("record{i:06} bucket{:03} Rust clipboard 项目笔记\nSynthetic text, no user clipboard or document. Deterministic benchmark body.", i % 100))],
        }).collect::<Vec<_>>();
        let items = store.insert_captures(&captures)?;
        for (index, item) in items.into_iter().enumerate() {
            if (batch * 250 + index) % 100 == 0 {
                pins.push(item.id);
            }
        }
        if (batch + 1) % 20 == 0 {
            println!(
                "Seeded {} / {ROWS}, {:.1}s",
                (batch + 1) * 250,
                started.elapsed().as_secs_f64()
            );
        }
    }
    let board = store.create_pinboard("Synthetic Benchmark", "#007aff")?;
    for chunk in pins.chunks(200) {
        store.pin_clips(board.id, chunk)?;
    }
    let metadata = json!({"kind":"pasters-history-benchmark-v1","rows":ROWS,"sourceCount":8,"deviceCount":3,"firstDevice":devices[0].id.to_string(),"boardSize":pins.len(),"seedSeconds":started.elapsed().as_secs_f64(),"seedVia":"insert_captures batches of 250; complete production sync outbox retained"});
    fs::write(
        directory.join("synthetic-fixture.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    drop(store);
    Ok(directory)
}

fn probe(root: &Path, source: &Path) -> Result<(), Box<dyn Error>> {
    let source = source.canonicalize()?;
    if source.parent() != Some(root) {
        return Err("probe accepts only synthetic artifact directories".into());
    }
    let metadata: Value =
        serde_json::from_slice(&fs::read(source.join("synthetic-fixture.json"))?)?;
    if metadata["kind"] != "pasters-history-benchmark-v1" {
        return Err("not a synthetic dataset".into());
    }
    let temporary = tempfile::Builder::new().prefix("probe-").tempdir_in(root)?;
    copy_database(
        &source.join("history.sqlite3"),
        &temporary.path().join("probe.sqlite3"),
    )?;
    let connection = rusqlite::Connection::open(temporary.path().join("probe.sqlite3"))?;
    let original = "SELECT c.id, bm25(clip_search) AS score FROM clip_search JOIN clips c ON c.id=clip_search.clip_id WHERE c.deleted_at_ms IS NULL AND clip_search MATCH 'clipboard*' ORDER BY score, c.last_copied_at_ms DESC, c.id DESC LIMIT 200";
    let covering = original.replace(
        "JOIN clips c ON",
        "JOIN clips c INDEXED BY probe_search_cover ON",
    );
    let mut reports = Vec::new();
    for (name, sql, setup) in [
        ("original", original, ""),
        (
            "fts_only_no_recency_DIAGNOSTIC",
            "SELECT clip_id, bm25(clip_search) FROM clip_search WHERE clip_search MATCH 'clipboard*' ORDER BY rank LIMIT 200",
            "",
        ),
        (
            "covering",
            covering.as_str(),
            "CREATE INDEX probe_search_cover ON clips(id,last_copied_at_ms,content_kind,source_bundle_id,device_id) WHERE deleted_at_ms IS NULL",
        ),
        (
            "covering_cache32m",
            covering.as_str(),
            "PRAGMA cache_size=-32768",
        ),
        (
            "covering_mmap256m",
            covering.as_str(),
            "PRAGMA mmap_size=268435456",
        ),
    ] {
        connection.execute_batch(setup)?;
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
            .query_map([], |r| r.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        let result = timed(name, || {
            let rows = connection
                .prepare(sql)?
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(rows.len(), 200);
            std::hint::black_box(rows);
            Ok(())
        })?;
        println!("{result} {plan:?}");
        reports.push(json!({"measurement":result,"plan":plan}));
    }
    fs::write(
        source.join("query-probe.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    Ok(())
}

fn validate_synthetic(root: &Path, source: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let source = source.canonicalize()?;
    if source.parent() != Some(root) {
        return Err("only synthetic artifact directories".into());
    }
    let metadata: Value =
        serde_json::from_slice(&fs::read(source.join("synthetic-fixture.json"))?)?;
    if metadata["kind"] != "pasters-history-benchmark-v1" || metadata["rows"] != ROWS {
        return Err("not a completed synthetic fixture".into());
    }
    Ok(source)
}

fn copy_database(source: &Path, target: &Path) -> Result<(), Box<dyn Error>> {
    // Do not open the baseline with SqliteStore: that would migrate it in place.
    // Online backup includes committed WAL pages without writing the source.
    let source =
        rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut target = rusqlite::Connection::open(target)?;
    rusqlite::backup::Backup::new(&source, &mut target)?.run_to_completion(
        128,
        std::time::Duration::from_millis(5),
        None,
    )?;
    Ok(())
}

fn probe_compact(root: &Path, source: &Path) -> Result<(), Box<dyn Error>> {
    let source = source.canonicalize()?;
    if source.parent() != Some(root) {
        return Err("only synthetic artifact directories".into());
    }
    let metadata: Value =
        serde_json::from_slice(&fs::read(source.join("synthetic-fixture.json"))?)?;
    if metadata["kind"] != "pasters-history-benchmark-v1" {
        return Err("not a synthetic fixture".into());
    }
    let temporary = tempfile::Builder::new()
        .prefix("probe-compact-")
        .tempdir_in(root)?;
    let target = temporary.path().join("probe.sqlite3");
    copy_database(&source.join("history.sqlite3"), &target)?;
    let connection = rusqlite::Connection::open(target)?;
    connection.execute_batch("CREATE TABLE probe_docs(doc_id INTEGER PRIMARY KEY, clip_id TEXT UNIQUE NOT NULL, last_copied_at_ms INTEGER NOT NULL, content_kind TEXT NOT NULL, source_bundle_id TEXT NOT NULL, device_id TEXT NOT NULL);
        INSERT INTO probe_docs SELECT s.rowid,c.id,c.last_copied_at_ms,c.content_kind,c.source_bundle_id,c.device_id FROM clip_search s JOIN clips c ON c.id=s.clip_id WHERE c.deleted_at_ms IS NULL;
        CREATE INDEX probe_docs_recent ON probe_docs(last_copied_at_ms DESC,clip_id DESC);")?;
    let queries = [
        (
            "compact_all_matches",
            "SELECT d.clip_id,bm25(clip_search) FROM clip_search JOIN probe_docs d ON d.doc_id=clip_search.rowid WHERE clip_search MATCH 'clipboard*' ORDER BY d.last_copied_at_ms DESC,d.clip_id DESC LIMIT 200",
        ),
        (
            "compact_rank_after_page_fts_outer",
            "WITH page AS MATERIALIZED (SELECT d.* FROM clip_search JOIN probe_docs d ON d.doc_id=clip_search.rowid WHERE clip_search MATCH 'clipboard*' ORDER BY d.last_copied_at_ms DESC,d.clip_id DESC LIMIT 200) SELECT p.clip_id,bm25(clip_search) FROM clip_search CROSS JOIN page p WHERE p.doc_id=clip_search.rowid AND clip_search MATCH 'clipboard*' ORDER BY p.last_copied_at_ms DESC,p.clip_id DESC",
        ),
        (
            "bounded_recent_rank_fts_outer",
            "WITH recent AS MATERIALIZED (SELECT * FROM probe_docs ORDER BY last_copied_at_ms DESC,clip_id DESC LIMIT 2048), page AS MATERIALIZED (SELECT r.* FROM recent r CROSS JOIN clip_search WHERE clip_search.rowid=r.doc_id AND clip_search MATCH 'clipboard*' ORDER BY r.last_copied_at_ms DESC,r.clip_id DESC LIMIT 200) SELECT p.clip_id,bm25(clip_search) FROM clip_search CROSS JOIN page p WHERE p.doc_id=clip_search.rowid AND clip_search MATCH 'clipboard*' ORDER BY p.last_copied_at_ms DESC,p.clip_id DESC",
        ),
    ];
    let mut reports = Vec::new();
    let mut expected = None;
    for (name, sql) in queries {
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
            .query_map([], |r| r.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;
        let result = timed(name, || {
            let rows = connection
                .prepare(sql)?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(rows.len(), 200);
            if let Some(expected) = &expected {
                assert_eq!(&rows, expected);
            } else {
                expected = Some(rows.clone());
            }
            std::hint::black_box(rows);
            Ok(())
        })?;
        println!("{result} {plan:?}");
        reports.push(json!({"measurement":result,"plan":plan}));
    }
    fs::write(
        source.join("compact-query-probe.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    Ok(())
}
