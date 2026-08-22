//! 端到端示例（与规范"使用方法"一致）。数据路径指向仓库内 tmp_data/。

use rust_bt::*;

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // 1. 加载信号
    let signal = load_signal("tmp_data/pred.csv")?; // CHANGE PATH IF NEEDED

    // 2. 策略参数
    let top_n = 100;
    let drop_n = 100;

    // 3. 资金与交易成本参数
    let cash = 10_000_000.0; // 1000 万
    let deal_price = "open";
    let open_cost = 0.00015; // 万 1.5
    let close_cost = 0.00065; // 万 6.5（佣金 + 卖出印花税）
    let min_cost = 5.0;
    let fixed_slippage = 0.01;
    let min_slippage_ratio = 0.0014;
    let volume_threshold = Some(0.5);
    let limit_threshold = Some(0.0985);

    // 4. 回测区间：闭开区间 [start_date, end_date），自动按交易日历对齐
    let start_date = "2026-01-01";
    let end_date = "2026-06-01";

    // 5. 加载行情与基准数据
    let data = BTData::new()
        .load_stock_bar("tmp_data/stock_bar.csv")?
        .load_benchmark("tmp_data/benchmark.csv")?
        .build()?;

    // 6. 账户 / 交易所 / 策略
    let account = Account::new(cash);
    let exchange = Exchange::new(
        deal_price,
        open_cost,
        close_cost,
        min_cost,
        fixed_slippage,
        min_slippage_ratio,
        volume_threshold,
        limit_threshold,
    )?;
    let strategy: Box<dyn Strategy> = Box::new(TopkDropoutStrategy::new(top_n, drop_n));

    // 7. 运行回测（with_progress 启用终端进度条，默认关闭）
    let mut backtest = Backtest::new(data, account, exchange, strategy).with_progress(true);
    let bt_result = backtest.run(&signal, start_date, end_date)?;

    // 8. 输出结果：统一写入 output/（已 gitignore，避免产物散落仓库根目录）
    std::fs::create_dir_all("output")?;
    bt_result.export_hist_position("output/hist_position.csv")?;
    bt_result.export_trades("output/trades.csv")?;
    let report = bt_result.gen_report("zz1000", "arithmetic")?;
    report.export_data("output/report_data.csv")?;
    report.plot("output/report_plot.html")?;

    println!("回测耗时: {:.2?}", bt_result.elapsed());
    println!("输出产物已写入 output/（hist_position / trades / report_data / report_plot）");
    println!("{:#?}", report.derived);

    Ok(())
}
