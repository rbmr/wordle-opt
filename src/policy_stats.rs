#![allow(clippy::needless_range_loop)]
//! Optional export of the periodic progress time series produced while
//! building a policy tree, for later analysis/plotting.
//!
//! This is deliberately a *small* table - one row per sampled interval, not per
//! node - so that collecting it cannot measurably slow a build down (see
//! `BuildOptions::collect_samples`). It is enough to plot nodes/sec, frontier
//! size, depth, and cache behaviour over the course of a build.
//!
//! The primary format is Parquet: columnar and compressed, so it loads directly
//! into pandas/polars/duckdb without the parse cost of CSV. A plain NDJSON
//! fallback (`--stats-format ndjson`) exists for quick inspection and for
//! environments where pulling in the Parquet codec is undesirable.

use crate::policy::BuildStats;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Metadata stamped onto an exported trace so a file can be traced back to the
/// tree it came from.
#[derive(Debug, Clone)]
pub struct StatsMeta {
    pub strategy: String,
    pub dictionary_hash: String,
    pub commit: String,
    pub num_guesses: u32,
    pub num_candidates: u32,
    pub root_candidates: u32,
}

/// The Parquet schema for the progress series.
const SCHEMA: &str = "
message wordle_policy_progress {
  REQUIRED DOUBLE elapsed_s;
  REQUIRED INT64 nodes;
  REQUIRED INT64 edges;
  REQUIRED INT64 frontier;
  REQUIRED INT32 depth;
  REQUIRED INT64 leaves;
  REQUIRED DOUBLE pick_ms_total;
  REQUIRED INT64 states_evaluated;
  REQUIRED INT64 guesses_evaluated;
  REQUIRED INT64 cache_hits;
  REQUIRED INT64 cache_misses;
  REQUIRED INT64 pruned_by_bounds;
  REQUIRED INT64 pruned_by_equivalence;
}";

fn metadata(meta: &StatsMeta, stats: &BuildStats) -> Vec<parquet::file::metadata::KeyValue> {
    use parquet::file::metadata::KeyValue;
    let s = &stats.summary;
    let kv = |k: &str, v: String| KeyValue::new(k.to_string(), v);
    vec![
        kv("wordle_opt.strategy", meta.strategy.clone()),
        kv("wordle_opt.dictionary_hash", meta.dictionary_hash.clone()),
        kv("wordle_opt.commit", meta.commit.clone()),
        kv("wordle_opt.num_guesses", meta.num_guesses.to_string()),
        kv("wordle_opt.num_candidates", meta.num_candidates.to_string()),
        kv(
            "wordle_opt.root_candidates",
            meta.root_candidates.to_string(),
        ),
        kv("wordle_opt.final_nodes", s.nodes.to_string()),
        kv("wordle_opt.final_edges", s.edges.to_string()),
        kv("wordle_opt.final_leaves", s.leaves.to_string()),
        kv("wordle_opt.final_max_depth", s.max_depth.to_string()),
        kv("wordle_opt.final_total_cost", s.total_cost.to_string()),
        kv("wordle_opt.final_total_ms", format!("{:.3}", s.total_ms)),
    ]
}

/// Writes the sampled progress series to `path` as a single-row-group Parquet
/// file. Columns are handed back by the low-level writer in schema order, so
/// the dispatch below is driven by a running column index.
pub fn write_parquet(path: &Path, stats: &BuildStats, meta: &StatsMeta) -> Result<(), String> {
    use parquet::basic::Compression;
    use parquet::column::writer::ColumnWriter;
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::parser::parse_message_type;
    use std::sync::Arc;

    let schema = Arc::new(parse_message_type(SCHEMA).map_err(|e| e.to_string())?);
    let props = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .set_key_value_metadata(Some(metadata(meta, stats)))
            .build(),
    );

    let file = File::create(path).map_err(|e| e.to_string())?;
    let mut writer = SerializedFileWriter::new(file, schema, props).map_err(|e| e.to_string())?;

    let elapsed_s: Vec<f64> = stats.samples.iter().map(|s| s.elapsed_s).collect();
    let nodes: Vec<i64> = stats.samples.iter().map(|s| s.nodes as i64).collect();
    let edges: Vec<i64> = stats.samples.iter().map(|s| s.edges as i64).collect();
    let frontier: Vec<i64> = stats.samples.iter().map(|s| s.frontier as i64).collect();
    let depth: Vec<i32> = stats.samples.iter().map(|s| s.depth as i32).collect();
    let leaves: Vec<i64> = stats.samples.iter().map(|s| s.leaves as i64).collect();
    let pick_ms_total: Vec<f64> = stats.samples.iter().map(|s| s.pick_ms_total).collect();
    let states_evaluated: Vec<i64> = stats
        .samples
        .iter()
        .map(|s| s.states_evaluated as i64)
        .collect();
    let guesses_evaluated: Vec<i64> = stats
        .samples
        .iter()
        .map(|s| s.guesses_evaluated as i64)
        .collect();
    let cache_hits: Vec<i64> = stats.samples.iter().map(|s| s.cache_hits as i64).collect();
    let cache_misses: Vec<i64> = stats
        .samples
        .iter()
        .map(|s| s.cache_misses as i64)
        .collect();
    let pruned_by_bounds: Vec<i64> = stats
        .samples
        .iter()
        .map(|s| s.pruned_by_bounds as i64)
        .collect();
    let pruned_by_equivalence: Vec<i64> = stats
        .samples
        .iter()
        .map(|s| s.pruned_by_equivalence as i64)
        .collect();

    let mut row_group = writer.next_row_group().map_err(|e| e.to_string())?;
    let mut col_index = 0usize;
    while let Some(mut col) = row_group.next_column().map_err(|e| e.to_string())? {
        let written = match (col_index, col.untyped()) {
            (0, ColumnWriter::DoubleColumnWriter(w)) => w.write_batch(&elapsed_s, None, None),
            (1, ColumnWriter::Int64ColumnWriter(w)) => w.write_batch(&nodes, None, None),
            (2, ColumnWriter::Int64ColumnWriter(w)) => w.write_batch(&edges, None, None),
            (3, ColumnWriter::Int64ColumnWriter(w)) => w.write_batch(&frontier, None, None),
            (4, ColumnWriter::Int32ColumnWriter(w)) => w.write_batch(&depth, None, None),
            (5, ColumnWriter::Int64ColumnWriter(w)) => w.write_batch(&leaves, None, None),
            (6, ColumnWriter::DoubleColumnWriter(w)) => w.write_batch(&pick_ms_total, None, None),
            (7, ColumnWriter::Int64ColumnWriter(w)) => w.write_batch(&states_evaluated, None, None),
            (8, ColumnWriter::Int64ColumnWriter(w)) => {
                w.write_batch(&guesses_evaluated, None, None)
            }
            (9, ColumnWriter::Int64ColumnWriter(w)) => w.write_batch(&cache_hits, None, None),
            (10, ColumnWriter::Int64ColumnWriter(w)) => w.write_batch(&cache_misses, None, None),
            (11, ColumnWriter::Int64ColumnWriter(w)) => {
                w.write_batch(&pruned_by_bounds, None, None)
            }
            (12, ColumnWriter::Int64ColumnWriter(w)) => {
                w.write_batch(&pruned_by_equivalence, None, None)
            }
            (i, _) => return Err(format!("unexpected parquet column {i}")),
        };
        written.map_err(|e| e.to_string())?;
        col.close().map_err(|e| e.to_string())?;
        col_index += 1;
    }
    row_group.close().map_err(|e| e.to_string())?;
    writer.close().map_err(|e| e.to_string())?;
    Ok(())
}

/// Writes the sampled progress series as newline-delimited JSON (a header
/// object, then one object per sample).
pub fn write_ndjson(path: &Path, stats: &BuildStats, meta: &StatsMeta) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    let mut w = BufWriter::new(file);
    let s = &stats.summary;
    let header = serde_json::json!({
        "type": "header",
        "strategy": meta.strategy,
        "dictionary_hash": meta.dictionary_hash,
        "commit": meta.commit,
        "num_guesses": meta.num_guesses,
        "num_candidates": meta.num_candidates,
        "root_candidates": meta.root_candidates,
        "final_nodes": s.nodes,
        "final_edges": s.edges,
        "final_leaves": s.leaves,
        "final_max_depth": s.max_depth,
        "final_total_cost": s.total_cost,
        "final_total_ms": s.total_ms,
    });
    writeln!(w, "{header}").map_err(|e| e.to_string())?;
    for sample in &stats.samples {
        let row = serde_json::json!({
            "type": "sample",
            "elapsed_s": sample.elapsed_s,
            "nodes": sample.nodes,
            "edges": sample.edges,
            "frontier": sample.frontier,
            "depth": sample.depth,
            "leaves": sample.leaves,
            "pick_ms_total": sample.pick_ms_total,
            "states_evaluated": sample.states_evaluated,
            "guesses_evaluated": sample.guesses_evaluated,
            "cache_hits": sample.cache_hits,
            "cache_misses": sample.cache_misses,
            "pruned_by_bounds": sample.pruned_by_bounds,
            "pruned_by_equivalence": sample.pruned_by_equivalence,
        });
        writeln!(w, "{row}").map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{BuildStats, BuildSummary, ProgressSample};

    fn sample(elapsed_s: f64, nodes: u64) -> ProgressSample {
        ProgressSample {
            elapsed_s,
            nodes,
            edges: nodes,
            frontier: 3,
            depth: 2,
            leaves: 1,
            pick_ms_total: elapsed_s * 1000.0,
            states_evaluated: nodes * 10,
            guesses_evaluated: nodes * 20,
            cache_hits: nodes,
            cache_misses: 1,
            pruned_by_bounds: nodes * 2,
            pruned_by_equivalence: nodes * 3,
        }
    }

    fn fixture() -> (BuildStats, StatsMeta) {
        let stats = BuildStats {
            samples: vec![sample(1.0, 100), sample(2.0, 250)],
            summary: BuildSummary {
                nodes: 250,
                edges: 249,
                leaves: 130,
                wins: 200,
                max_depth: 4,
                total_cost: 700,
                total_ms: 2000.0,
                pick_ms_total: 1500.0,
                states_evaluated: 2500,
                guesses_evaluated: 5000,
                cache_hits: 250,
                pruned_by_bounds: 500,
                pruned_by_equivalence: 750,
            },
        };
        let meta = StatsMeta {
            strategy: "optimal".to_string(),
            dictionary_hash: "0xdeadbeef".to_string(),
            commit: "abc1234".to_string(),
            num_guesses: 14855,
            num_candidates: 200,
            root_candidates: 200,
        };
        (stats, meta)
    }

    #[test]
    fn parquet_roundtrip_reads_back() {
        use parquet::file::reader::{FileReader, SerializedFileReader};

        let (stats, meta) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("progress.parquet");
        write_parquet(&path, &stats, &meta).unwrap();

        let reader = SerializedFileReader::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(reader.metadata().file_metadata().num_rows(), 2);

        // The stamped metadata must survive, so a trace can be traced back.
        let kv = reader
            .metadata()
            .file_metadata()
            .key_value_metadata()
            .unwrap();
        let get = |k: &str| kv.iter().find(|p| p.key == k).and_then(|p| p.value.clone());
        assert_eq!(get("wordle_opt.strategy").as_deref(), Some("optimal"));
        assert_eq!(get("wordle_opt.final_nodes").as_deref(), Some("250"));
        assert_eq!(get("wordle_opt.final_total_cost").as_deref(), Some("700"));
    }

    #[test]
    fn ndjson_has_header_and_rows() {
        let (stats, meta) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("progress.ndjson");
        write_ndjson(&path, &stats, &meta).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 samples
        assert!(lines[0].contains("\"type\":\"header\""));
        assert!(lines[1].contains("\"type\":\"sample\""));
        assert!(lines[2].contains("\"nodes\":250"));
    }
}
