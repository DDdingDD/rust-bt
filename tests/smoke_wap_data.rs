//! WAP 时段数据端到端冒烟测试：用仓库内 tmp_data/wap.parquet 跑通 vwap11/twap11 回测。
//!
//! 仅校验装配、运行、导出无报错；**不做数值对拍**（tmp_data 仅作格式参考）。
//! wap.parquet 缺失时自动跳过。该文件约 2.7GB，调试模式加载极慢，请在 release 下运行：
//! `cargo test --release --test smoke_wap_data`

use rust_bt::*;
use tempfile::TempDir;

fn wap_data_exists() -> bool {
    std::path::Path::new("tmp_data/wap.parquet").exists()
        && std::path::Path::new("tmp_data/stock_bar.csv").exists()
        && std::path::Path::new("tmp_data/pred.csv").exists()
        && std::path::Path::new("tmp_data/benchmark.csv").exists()
}

fn smoke_with_deal_price(deal_price: &str) {
    if !wap_data_exists() {
        eprintln!("tmp_data/wap.parquet 不存在，跳过 WAP 冒烟测试");
        return;
    }

    let window = deal_price
        .trim_start_matches("vwap")
        .trim_start_matches("twap")
        .parse::<u8>()
        .expect("deal_price 应含窗口号");

    let signal = load_signal("tmp_data/pred.csv").unwrap();
    let data = BTData::new()
        .load_stock_bar("tmp_data/stock_bar.csv")
        .unwrap()
        .load_wap("tmp_data/wap.parquet", window)
        .unwrap()
        .load_benchmark("tmp_data/benchmark.csv")
        .unwrap()
        .build()
        .unwrap();
    let account = Account::new(10_000_000.0);
    let exchange = Exchange::new(
        deal_price, 0.00015, 0.00065, 5.0, 0.01, 0.0014, Some(0.5), Some(0.0985),
    )
    .unwrap();
    let strategy: Box<dyn Strategy> = Box::new(TopkDropoutStrategy::new(100, 100));
    let mut bt = Backtest::new(data, account, exchange, strategy).unwrap();
    let result = bt.run(&signal, "2022-10-10", "2022-11-01").unwrap();

    let dir = TempDir::new().unwrap();
    result.export_trades(dir.path().join("trades.csv").to_str().unwrap()).unwrap();
    let report = result.gen_report("zz1000", "arithmetic").unwrap();
    report.export_data(dir.path().join("report_data.csv").to_str().unwrap()).unwrap();

    assert!(result.trades().len() > 0, "{deal_price} 冒烟区间应有成交");
    assert!(report.derived.annualized_return.is_finite());
}

#[test]
fn smoke_vwap11() {
    smoke_with_deal_price("vwap11");
}

#[test]
fn smoke_twap11() {
    smoke_with_deal_price("twap11");
}
