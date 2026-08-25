//! 端到端冒烟测试（架构 §8 第 3 层）：用仓库内 tmp_data/ 跑通完整回测。
//!
//! tmp_data 仅作格式与规模参考：全区间无报错、无 panic，校验三个输出文件的
//! 列名与日期格式（YYYY-MM-DD）；**不做数值对拍**。数据缺失时自动跳过。

use rust_bt::*;
use tempfile::TempDir;

fn tmp_data_exists() -> bool {
    std::path::Path::new("tmp_data/stock_bar.csv").exists()
        && std::path::Path::new("tmp_data/pred.csv").exists()
        && std::path::Path::new("tmp_data/benchmark.csv").exists()
}

#[test]
fn smoke_tmp_data() {
    if !tmp_data_exists() {
        eprintln!("tmp_data 不存在，跳过冒烟测试");
        return;
    }

    let signal = load_signal("tmp_data/pred.csv").unwrap();
    let data = BTData::new()
        .load_stock_bar("tmp_data/stock_bar.csv")
        .unwrap()
        .load_benchmark("tmp_data/benchmark.csv")
        .unwrap()
        .build()
        .unwrap();
    let account = Account::new(10_000_000.0);
    let exchange = Exchange::new(
        "open", 0.00015, 0.00065, 5.0, 0.01, 0.0014, Some(0.5), Some(0.0985),
    )
    .unwrap();
    let strategy: Box<dyn Strategy> = Box::new(TopkDropoutStrategy::new(100, 100));
    let mut bt = Backtest::new(data, account, exchange, strategy).unwrap();
    // pred.csv 自 2022-10-10 起；取一个月区间控制耗时
    let result = bt.run(&signal, "2022-10-10", "2022-11-01").unwrap();

    let dir = TempDir::new().unwrap();
    let hist = dir.path().join("hist_position.csv");
    let trades = dir.path().join("trades.csv");
    let report_data = dir.path().join("report_data.csv");
    result.export_hist_position(hist.to_str().unwrap()).unwrap();
    result.export_trades(trades.to_str().unwrap()).unwrap();
    let report = result.gen_report("zz1000", "arithmetic").unwrap();
    report.export_data(report_data.to_str().unwrap()).unwrap();

    let date_re = |s: &str| {
        let b = s.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b[..4].iter().all(|c| c.is_ascii_digit())
            && b[5..7].iter().all(|c| c.is_ascii_digit())
            && b[8..10].iter().all(|c| c.is_ascii_digit())
    };
    let check = |path: &std::path::Path, header: &str, date_col: usize| {
        let text = std::fs::read_to_string(path).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), header, "{} 列名", path.display());
        let mut n = 0;
        for line in lines {
            let cols: Vec<&str> = line.split(',').collect();
            assert!(date_re(cols[date_col]), "{} 日期格式: {}", path.display(), cols[date_col]);
            n += 1;
        }
        n
    };

    let n_hist = check(
        &hist,
        "datetime,instrument,volume,cost_price,price,weight,count_day",
        0,
    );
    let n_trades = check(
        &trades,
        "datetime,instrument,side,volume,price,deal_volume,deal_price,deal_cost",
        0,
    );
    let n_report = check(
        &report_data,
        "datetime,account,return,total_turnover,turnover,total_cost,cost,value,cash,benchmark",
        0,
    );

    // 区间约 15 个交易日：报表逐日一行；应有成交与持仓
    assert_eq!(n_report, result.daily().len());
    assert!(n_trades > 0, "冒烟区间应有成交");
    assert!(n_hist > 0, "冒烟区间应有持仓");
    // 净值曲线应可绘制（不写盘，仅验证衍生指标有限）
    assert!(report.derived.annualized_return.is_finite());
}
