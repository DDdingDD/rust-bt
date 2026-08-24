//! `bt <config.yml>`：YAML 配置驱动的回测 CLI。
//!
//! 配置格式见仓库根目录 config.example.yml；除数据路径与回测区间外均有默认值
//! （对齐 examples/run_backtest.rs）。装配与运行复用嵌入 API `api::run`，
//! 本文件只负责配置加载、信号加载与产物导出。

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
    // 枚举参数在 to_params 解析（加载期校验已过，fail fast 于行情加载前）
    let params = cfg.to_params()?;

    // 信号独立于 BtParams（嵌入方可用内存构造，CLI 从文件加载）
    let signal = load_signal(cfg.signal())?;

    let output = run(params, &signal)?;

    let names = ExportNames {
        hist_position: cfg.output.hist_position.clone(),
        trades: cfg.output.trades.clone(),
        report_data: cfg.output.report_data.clone(),
        report_plot: cfg.output.report_plot.clone(),
    };
    output.export_all(&cfg.output.dir, &names)?;

    println!("回测耗时: {:.2?}", output.result.elapsed());
    println!(
        "输出产物已写入 {}/（{} / {} / {} / {}）",
        cfg.output.dir, names.hist_position, names.trades, names.report_data, names.report_plot
    );
    println!("\n{}", output.report.summary());

    Ok(())
}
