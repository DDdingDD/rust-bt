//! parquet 数据输入集成测试：stock_bar / benchmark 以 parquet 提供时，
//! 全链路（加载、校验、撮合、估值、报告）与 CSV 输入完全一致。
//!
//! 对拍方式：同一合成场景（与 acceptance_basic 的 build_and_rebalance 同构）
//! 写出 CSV 后原样转为 parquet（仅换物理存储，列内容不变），组件层跑 CSV、
//! 嵌入 API 跑 parquet，逐日账户与逐笔成交必须完全一致。

mod common;

use std::path::Path;

use common::*;
use polars::prelude::*;
use rust_bt::load_signal;
use rust_bt::{api, data::StockBarStore, BtParams, DataPaths, DataSource, DealPrice, ExchangeParams, StrategySpec};
use tempfile::TempDir;

const D: [&str; 5] = [
    "2026-01-05",
    "2026-01-06",
    "2026-01-07",
    "2026-01-08",
    "2026-01-09",
];

/// 与 acceptance_basic 相同的场景：3 只股票、5 个交易日、零成本零滑点。
fn setup() -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            bars.push(Bar::new(d, inst));
        }
    }
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            (D[0], "SH600001", 3.0),
            (D[0], "SH600002", 2.0),
            (D[0], "SZ000001", 1.0),
            (D[2], "SH600001", 1.0),
            (D[2], "SH600002", 2.0),
            (D[2], "SZ000001", 3.0),
        ],
    );
    write_bench(dir.path(), &bench_rows(&D, 0.001));
    let params = Params {
        with_benchmark: true,
        ..Default::default()
    };
    (dir, params)
}

/// CSV -> parquet 物理格式转换（列内容不变）。
fn csv_to_parquet(src: &Path, dst: &Path) {
    let mut df = CsvReadOptions::default()
        .with_has_header(true)
        .try_into_reader_with_file_path(Some(src.to_path_buf()))
        .unwrap()
        .finish()
        .unwrap();
    let file = std::fs::File::create(dst).unwrap();
    ParquetWriter::new(file).finish(&mut df).unwrap();
}

/// 嵌入 API 参数（对齐 common::Params 默认：零成本零滑点，top_n=2，drop_n=1）。
fn api_params(dir: &TempDir, stock_bar: &str, benchmark: &str) -> BtParams {
    BtParams {
        data: DataSource::Paths(DataPaths {
            stock_bar: dir.path().join(stock_bar).to_str().unwrap().into(),
            benchmark: dir.path().join(benchmark).to_str().unwrap().into(),
            wap: None,
        }),
        start_date: "2026-01-05".into(),
        end_date: "2026-01-10".into(),
        initial_cash: 100_000.0,
        strategy: StrategySpec::topk_dropout(2, 1),
        exchange: ExchangeParams {
            deal_price: DealPrice::Open,
            open_cost: 0.0,
            close_cost: 0.0,
            min_cost: 0.0,
            fixed_slippage: 0.0,
            min_slippage_ratio: 0.0,
            volume_threshold: None,
            limit_threshold: Some(0.0985),
        },
        benchmark_name: rust_bt::BenchmarkName::Zz1000,
        excess_method: rust_bt::ExcessMethod::Arithmetic,
        progress: false,
    }
}

#[test]
fn parquet_inputs_match_csv_end_to_end() {
    let (dir, params) = setup();
    let expected = run_bt(&dir, &params);

    csv_to_parquet(
        &dir.path().join("stock_bar.csv"),
        &dir.path().join("stock_bar.parquet"),
    );
    csv_to_parquet(
        &dir.path().join("benchmark.csv"),
        &dir.path().join("benchmark.parquet"),
    );

    let signal = load_signal(dir.path().join("pred.csv").to_str().unwrap()).unwrap();
    let output = api::run(
        api_params(&dir, "stock_bar.parquet", "benchmark.parquet"),
        &signal,
    )
    .unwrap();

    assert_daily_same(&output.result, &expected);
    assert_trades_same(&output.result, &expected);
    assert_eq!(
        output.result.hist_positions().len(),
        expected.hist_positions().len(),
        "hist_positions 行数"
    );

    // 报告序列与组件层 gen_report 一致（基准 parquet -> 基准收益一致）
    let report = expected.gen_report("zz1000", "arithmetic").unwrap();
    assert_eq!(output.report.dates(), report.dates());
    assert_eq!(output.report.cum_with_cost(), report.cum_with_cost());
    assert_eq!(output.report.cum_benchmark(), report.cum_benchmark());
    assert_eq!(output.report.cum_excess(), report.cum_excess());
}

/// `.pq` 扩展名同样识别为 parquet。
#[test]
fn pq_extension_recognized() {
    let (dir, _) = setup();
    csv_to_parquet(
        &dir.path().join("stock_bar.csv"),
        &dir.path().join("stock_bar.pq"),
    );
    let store = StockBarStore::load(&dir.path().join("stock_bar.pq")).unwrap();
    assert_eq!(store.calendar.len(), 5);
    assert_eq!(store.codes.len(), 15);
    assert!(store.code_set.contains(&600001));
}
