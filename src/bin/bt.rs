//! `bt <config.yml>`：YAML 配置驱动的回测 CLI。
//!
//! 配置格式见仓库根目录 config.example.yml；除数据路径与回测区间外均有默认值
//! （对齐 examples/run_backtest.rs）。

use std::path::Path;

use rust_bt::*;

fn main() -> anyhow::Result<()> {
    let config_path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("用法: bt <config.yml>");
            std::process::exit(2);
        }
    };

    env_logger::init();

    let cfg = BtConfig::load(&config_path)?;

    // 1. 账户 / 交易所 / 策略（Exchange::new 的费用 / 阈值校验先于数据加载执行，
    //    参数错误时 fail fast，不必等数百 MB 行情加载完）
    let account = Account::new(cfg.account.initial_cash);
    let exchange = Exchange::new(
        &cfg.exchange.deal_price,
        cfg.exchange.open_cost,
        cfg.exchange.close_cost,
        cfg.exchange.min_cost,
        cfg.exchange.fixed_slippage,
        cfg.exchange.min_slippage_ratio,
        cfg.exchange.volume_threshold,
        cfg.exchange.limit_threshold,
    )?;
    let strategy: Box<dyn Strategy> = match cfg.strategy.name.as_str() {
        "topk_dropout" => Box::new(
            TopkDropoutStrategy::new(cfg.strategy.top_n, cfg.strategy.drop_n)
                .with_only_tradable(cfg.strategy.only_tradable)
                .with_forbid_st(cfg.strategy.forbid_st),
        ),
        // BtConfig::load 已校验，此处不可达
        other => anyhow::bail!("未知策略: {other}"),
    };

    // 2. 加载信号
    let signal = load_signal(cfg.signal())?;

    // 3. 加载行情与基准数据
    let data = BTData::new()
        .load_stock_bar(cfg.stock_bar())?
        .load_benchmark(cfg.benchmark_data())?
        .build()?;

    // 4. 运行回测
    let mut backtest = Backtest::new(data, account, exchange, strategy).with_progress(cfg.progress);
    let bt_result = backtest.run(&signal, cfg.start_date(), cfg.end_date())?;

    // 5. 输出结果
    let out_dir = Path::new(&cfg.output.dir);
    std::fs::create_dir_all(out_dir)?;
    let out = |name: &str| out_dir.join(name).to_string_lossy().into_owned();
    bt_result.export_hist_position(&out(&cfg.output.hist_position))?;
    bt_result.export_trades(&out(&cfg.output.trades))?;
    let report = bt_result.gen_report(&cfg.report.benchmark, &cfg.report.excess_method)?;
    report.export_data(&out(&cfg.output.report_data))?;
    report.plot(&out(&cfg.output.report_plot))?;

    println!("回测耗时: {:.2?}", bt_result.elapsed());
    println!(
        "输出产物已写入 {}/（{} / {} / {} / {}）",
        cfg.output.dir,
        cfg.output.hist_position,
        cfg.output.trades,
        cfg.output.report_data,
        cfg.output.report_plot
    );
    println!("{:#?}", report.derived);

    Ok(())
}
