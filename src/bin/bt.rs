//! `bt <config.yml> [更多配置.yml ...]`：YAML 配置驱动的回测 CLI。
//!
//! 配置格式见仓库根目录 config.example.yml；除数据路径与回测区间外均有默认值
//! （对齐 examples/run_backtest.rs）。装配与运行复用嵌入 API `api::run`，
//! 本文件只负责配置加载、信号加载与产物导出。
//!
//! 传入多份配置时，各配置的 data 路径须一致（wap 模式配置还须同一时段，
//! `BtConfig::check_shareable_data` 前置校验），数据只加载一次，逐配置回测
//! 并各自导出（参数扫描免重载，决策 D15）。

use rust_bt::*;

fn main() -> anyhow::Result<()> {
    let config_paths: Vec<String> = std::env::args().skip(1).collect();
    if config_paths.is_empty() {
        eprintln!("用法: bt <config.yml> [更多配置.yml ...]");
        eprintln!("      bt --version | -V        显示版本号");
        std::process::exit(2);
    }
    // 版本号取 Cargo.toml（发布 tag 与之一致，见记忆 release-versioning）
    if config_paths[0] == "--version" || config_paths[0] == "-V" {
        println!("bt {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    env_logger::init();

    let cfgs: Vec<BtConfig> = config_paths
        .iter()
        .map(|p| BtConfig::load(p))
        .collect::<anyhow::Result<_>>()?;
    let refs: Vec<(&str, &BtConfig)> = config_paths
        .iter()
        .map(String::as_str)
        .zip(cfgs.iter())
        .collect();
    BtConfig::check_shareable_data(&refs)?;

    // 数据只加载一次（wap 时段取 wap 模式配置的统一窗口，校验已保证一致；
    // 非 wap 配置共享到含 wap 的数据时由装配层告警忽略）
    let mut data = BTData::new().load_stock_bar(cfgs[0].stock_bar())?;
    if let Some(wap_cfg) = cfgs.iter().find(|c| {
        matches!(
            DealPrice::parse(&c.exchange.deal_price),
            Ok(DealPrice::Wap { .. })
        )
    }) {
        let window = match DealPrice::parse(&wap_cfg.exchange.deal_price)? {
            DealPrice::Wap { window, .. } => window,
            _ => unreachable!("find 已匹配 Wap 分支"),
        };
        let wap_path = wap_cfg
            .data
            .wap
            .as_deref()
            .expect("load 已校验 wap 模式必提供 data.wap");
        data = data.load_wap(wap_path, window)?;
    }
    let data = data.load_benchmark(cfgs[0].benchmark_data())?.build()?;

    for (path, cfg) in config_paths.iter().zip(&cfgs) {
        if cfgs.len() > 1 {
            println!("=== {path} ===");
        }
        // 枚举参数在 to_params 解析（加载期校验已过，fail fast 于回测前）
        let mut params = cfg.to_params()?;
        params.data = DataSource::Shared(data.clone());

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
    }

    Ok(())
}
