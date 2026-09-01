//! 嵌入 API 示例（与 doc/api.md 的示例集同构）：内存信号 + 自定义策略 + 内存消费 + 导出。
//!
//! 数据路径指向仓库内 tmp_data/（与 examples/run_backtest.rs 一致，可按需修改）。

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_bt::{
    run, signal_from_pairs, BTData, BtParams, DataSource, Decision, ExchangeParams, ExportNames,
    Order, Result, Strategy, StrategyContext, StrategySpec,
};

/// 全仓轮动单只最高分股票（doc/api.md §8 的自定义策略示例）。
pub struct Top1Rotation;

impl Strategy for Top1Rotation {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
        // 1. 目标 = 当日 score 最高且可买入的股票
        let target = ctx
            .signal
            .codes
            .iter()
            .copied()
            .zip(ctx.signal.scores.iter().copied())
            .filter(|(c, _)| ctx.tradable.get(*c).is_some_and(|t| t.buyable()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(code, _)| code);
        let Some(target) = target else {
            return Ok(Decision::default()); // 无可买候选，今日不动作
        };

        // 2. 卖出全部非目标持仓（不可交易的自然卖不掉，留在持仓）
        let mut sell_orders = Vec::new();
        for (&code, entry) in ctx.positions.iter() {
            if code == target || !ctx.tradable.get(code).is_some_and(|t| t.sellable()) {
                continue;
            }
            let price = ctx.tradable.get(code).unwrap().deal_price;
            sell_orders.push(Order::new(code, -entry.volume, price));
        }

        // 3. 全仓买入目标（现金 + 预期回款毛额，按 100 股整手向下取整）
        let mut buy_orders = Vec::new();
        if !ctx.positions.contains_key(&target) {
            let t = ctx.tradable.get(target).unwrap();
            let proceeds: f64 = sell_orders
                .iter()
                .map(|o| -o.volume * ctx.tradable.get(o.stock).unwrap().deal_price)
                .sum();
            let budget = ctx.cash + proceeds;
            let lots = (budget / t.deal_price / 100.0).floor();
            if lots > 0.0 {
                buy_orders.push(Order::new(target, lots * 100.0, t.deal_price));
            }
        }

        Ok(Decision {
            sell_orders,
            buy_orders,
            target_positions: Some(1),
        })
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // 1. 内存信号：由你的因子模型生成（此处从 pred.csv 读出的等价演示--
    //    实际嵌入时来源可以是任意计算过程）
    let from_csv = rust_bt::load_signal("tmp_data/pred.csv")?;
    let mut days: BTreeMap<NaiveDate, Vec<(String, f64)>> = BTreeMap::new();
    for date in from_csv.dates() {
        for (code, score) in from_csv.get(&date).unwrap().as_map() {
            days.entry(date).or_default().push((
                rust_bt::format_instrument(code)?,
                score,
            ));
        }
    }
    let signal = signal_from_pairs(days)?;

    // 2. 数据加载一次，多次回测共享（参数扫描免重载，决策 D15）；
    //    只需单次回测时也可用 DataSource::Paths 由 run 内部自行加载
    let data = BTData::new()
        .load_stock_bar("tmp_data/stock_bar.csv")? // 股票日行情（交易日历来源；CSV 或 parquet）
        .load_benchmark("tmp_data/benchmark.csv")? // 基准收益（报告必需）
        // .load_wap("tmp_data/wap.parquet", 11)?  // deal_price = vwapN/twapN 时按对应时段加载
        .build()?;

    // 参数构造闭包：data 共享克隆（Arc，廉价），其余参数按需变化
    let params = |strategy: StrategySpec| BtParams {
        data: DataSource::Shared(data.clone()),
        start_date: "2026-01-01".into(), // 区间 [start, end)，按交易日历自动对齐
        end_date: "2026-06-01".into(),
        initial_cash: 10_000_000.0,
        strategy,
        exchange: ExchangeParams::default(),
        benchmark_name: rust_bt::BenchmarkName::Zz1000,
        excess_method: rust_bt::ExcessMethod::Arithmetic,
        progress: false,
    };

    // 3. 一次调用 + 内存消费（summary 为关键指标简报；逐日序列见 doc/api.md §6）
    let output = run(
        params(StrategySpec::Custom(Box::new(Top1Rotation))),
        &signal,
    )?;
    println!("回测耗时: {:.2?}", output.result.elapsed());
    println!("{}", output.report.summary());
    // 序列自定义指标示例：卡玛比率（年化收益 / 最大回撤）
    println!(
        "卡玛比率: {:.4}",
        output.report.derived.annualized_return / output.report.derived.max_drawdown
    );

    // 4. 可选导出（与 CLI 产物一致）
    std::fs::create_dir_all("output")?;
    output.export_all("output", &ExportNames::default())?;
    println!("产物已写入 output/");

    // 5. 同一份数据继续做参数扫描（数据不重载，仅主循环耗时）
    for top_n in [50, 100] {
        let out = run(params(StrategySpec::topk_dropout(top_n, top_n)), &signal)?;
        println!("top_n={top_n:>3}  sharpe={:.4}", out.report.derived.sharpe);
    }

    Ok(())
}
