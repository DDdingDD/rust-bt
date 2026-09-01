//! 嵌入 API（高层便捷层，架构 §4.10）：一次调用完成装配与回测。
//!
//! 与组件 Facade（`BTData` / `Account` / `Exchange` / `Backtest`）的关系：
//! 本模块把"装配 + 运行 + 报告"折叠为 [`run`] 一个入口，参数全部类型化
//! （`DealPrice` / `BenchmarkName` / `ExcessMethod` 为枚举，编译期杜绝拼写错误），
//! 适合嵌入其他 Rust 代码；需要进度条控制之外的细粒度编排（如自定义流程）
//! 时仍可直接使用组件层，两层共用同一撮合与估值路径，口径一致。
//!
//! 信号支持多种来源：`load_signal(path)` 读 CSV、`signal_from_dataframe(&df)`
//! 直连 polars DataFrame、或 `Signal::from_days(...)` 在内存中程序化构造
//! （嵌入方自行生成信号的研究循环）。

use std::collections::BTreeMap;
use std::path::Path;

use chrono::NaiveDate;

use crate::account::Account;
use crate::backtest::Backtest;
use crate::data::BTData;
use crate::error::{BtError, Result};
use crate::exchange::Exchange;
use crate::report::Report;
use crate::result::BTResult;
use crate::signal::{Signal, SignalDay};
use crate::strategy::{Strategy, TopkDropoutStrategy, TopkStrategy};
use crate::types::{BenchmarkName, DealPrice, ExcessMethod};

/// 数据文件路径（stock_bar / benchmark 必填；wap 在 `deal_price` 为 vwapN/twapN 时必填）。
#[derive(Debug, Clone)]
pub struct DataPaths {
    /// 股票日行情 CSV / parquet 路径（stock_bar，交易日历来源）。
    pub stock_bar: String,
    /// 基准收益 CSV / parquet 路径（benchmark，报告必需）。
    pub benchmark: String,
    /// wap 时段数据路径（CSV 或 parquet，按扩展名识别）；`deal_price` 为
    /// vwapN/twapN 时必填，时段号按 deal_price 推导。
    pub wap: Option<String>,
}

/// 数据来源：每次 `run` 重新加载，或共享一份已加载数据多次复用（决策 D15）。
#[derive(Debug, Clone)]
pub enum DataSource {
    /// 从路径加载：每次 `run` 重新读取并校验数据文件。
    Paths(DataPaths),
    /// 共享已加载的 `BTData`（内部 `Arc`，`clone` 廉价）：参数扫描等多次
    /// 回测场景只加载一次。wap 时段号在 `load_wap` 时固定，各次 run 的
    /// `deal_price` 时段须与之一致（装配期校验，不一致报错）。
    Shared(BTData),
}

/// 高层回测参数。数据来源经 `data` 字段指定（路径或共享数据），
/// 信号经 `signal` 参数注入（文件或内存构造均可）。
#[derive(Debug)]
pub struct BtParams {
    /// 行情 / 基准 / wap 数据来源。
    pub data: DataSource,
    /// 回测区间（闭开区间 [start, end)，按交易日历自动对齐）。
    pub start_date: String,
    pub end_date: String,
    /// 期初资金（须为正的有限值）。
    pub initial_cash: f64,
    /// 策略：内置 topk_dropout / topk 参数化，或注入 `Box<dyn Strategy>` 自定义实现。
    pub strategy: StrategySpec,
    /// 撮合与成本参数（默认值对齐 `config.example.yml`）。
    pub exchange: ExchangeParams,
    /// 报告基准名称。
    pub benchmark_name: BenchmarkName,
    /// 超额收益口径。
    pub excess_method: ExcessMethod,
    /// 终端进度条（渲染到 stderr，嵌入场景通常关闭）。
    pub progress: bool,
}

/// 策略规格：内置 topk_dropout / topk 或自定义策略注入。
pub enum StrategySpec {
    /// 内置 topk_dropout（参数语义同 `TopkDropoutStrategy`）。
    TopkDropout {
        top_n: usize,
        drop_n: usize,
        only_tradable: bool,
        forbid_st: bool,
    },
    /// 内置 topk（参数语义同 `TopkStrategy`）：每日持有 score 前 top_n 只。
    Topk { top_n: usize, forbid_st: bool },
    /// 自定义策略（`run` 会消耗该 Box 装配进 `Backtest`，如需复用请自行重建）。
    Custom(Box<dyn Strategy>),
}

impl std::fmt::Debug for StrategySpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TopkDropout {
                top_n,
                drop_n,
                only_tradable,
                forbid_st,
            } => f
                .debug_struct("TopkDropout")
                .field("top_n", top_n)
                .field("drop_n", drop_n)
                .field("only_tradable", only_tradable)
                .field("forbid_st", forbid_st)
                .finish(),
            Self::Topk { top_n, forbid_st } => f
                .debug_struct("Topk")
                .field("top_n", top_n)
                .field("forbid_st", forbid_st)
                .finish(),
            Self::Custom(_) => f.debug_tuple("Custom").field(&"<dyn Strategy>").finish(),
        }
    }
}

impl StrategySpec {
    /// topk_dropout 快捷方式（`only_tradable` / `forbid_st` 默认 false）。
    pub fn topk_dropout(top_n: usize, drop_n: usize) -> Self {
        Self::TopkDropout {
            top_n,
            drop_n,
            only_tradable: false,
            forbid_st: false,
        }
    }

    /// topk 快捷方式（`forbid_st` 默认 false）。
    pub fn topk(top_n: usize) -> Self {
        Self::Topk {
            top_n,
            forbid_st: false,
        }
    }
}

/// 撮合与成本参数。默认值与 `config.rs` / `config.example.yml` 一致，
/// 修改任一处须同步另一处。
#[derive(Clone, Copy, Debug)]
pub struct ExchangeParams {
    pub deal_price: DealPrice,
    /// 买入费率（默认万 1.5）。
    pub open_cost: f64,
    /// 卖出费率（佣金 + 印花税，默认万 6.5）。
    pub close_cost: f64,
    /// 单笔最低费用（默认 5 元）。
    pub min_cost: f64,
    /// 固定滑点（默认 0.01）。
    pub fixed_slippage: f64,
    /// 最小滑点比例（默认 0.0014）。
    pub min_slippage_ratio: f64,
    /// 成交量限制比例（None 不限制）。
    pub volume_threshold: Option<f64>,
    /// 涨跌停判定阈值（None 不限制）。
    pub limit_threshold: Option<f64>,
}

impl Default for ExchangeParams {
    fn default() -> Self {
        Self {
            deal_price: DealPrice::Open,
            open_cost: 0.00015,
            close_cost: 0.00065,
            min_cost: 5.0,
            fixed_slippage: 0.01,
            min_slippage_ratio: 0.0014,
            volume_threshold: Some(0.5),
            limit_threshold: Some(0.0985),
        }
    }
}

/// 回测输出：逐日账户 / 成交 / 持仓（`result`）与报告指标（`report`）。
/// 文件导出见 [`BtOutput::export_all`]。
pub struct BtOutput {
    pub result: BTResult,
    pub report: Report,
}

impl std::fmt::Debug for BtOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BtOutput")
            .field("result", &"<BTResult>")
            .field("report", &"<Report>")
            .finish()
    }
}

/// 导出产物文件名（相对输出目录）。
#[derive(Debug, Clone)]
pub struct ExportNames {
    pub hist_position: String,
    pub trades: String,
    pub report_data: String,
    pub report_plot: String,
}

impl Default for ExportNames {
    fn default() -> Self {
        Self {
            hist_position: "hist_position.csv".into(),
            trades: "trades.csv".into(),
            report_data: "report_data.csv".into(),
            report_plot: "report_plot.html".into(),
        }
    }
}

impl BtOutput {
    /// 导出全部四产物到 `dir`（不存在时自动创建）：hist_position / trades /
    /// report_data / report_plot。文件名可用 [`ExportNames`] 定制。
    pub fn export_all(&self, dir: impl AsRef<Path>, names: &ExportNames) -> Result<()> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        self.result.export_hist_position(&path(&names.hist_position))?;
        self.result.export_trades(&path(&names.trades))?;
        self.report.export_data(&path(&names.report_data))?;
        self.report.plot(&path(&names.report_plot))?;
        Ok(())
    }
}

/// 运行回测：装配（账户 / 交易所 / 策略）-> 数据 -> 主循环 -> 报告。
///
/// 参数校验先于数据加载（数百 MB 行情读取前 fail fast）；`params` 按值传入
/// （自定义策略 Box 被消耗）。`params.data` 为 [`DataSource::Paths`] 时每次调用
/// 重新加载数据文件；为 [`DataSource::Shared`] 时复用已加载的 `BTData`（内部
/// `Arc`，参数扫描多次调用只加载一次，派生列仍按每次撮合参数重建）。
pub fn run(params: BtParams, signal: &Signal) -> Result<BtOutput> {
    // 1. 数值参数校验（嵌入方不经过 BtConfig，须在此拦截）
    if !params.initial_cash.is_finite() || params.initial_cash <= 0.0 {
        return Err(BtError::InvalidParam(format!(
            "initial_cash 须为正数，收到: {}",
            params.initial_cash
        )));
    }
    let bad_top_n = match &params.strategy {
        StrategySpec::TopkDropout { top_n, .. } | StrategySpec::Topk { top_n, .. } => {
            Some(*top_n).filter(|n| *n < 1)
        }
        StrategySpec::Custom(_) => None,
    };
    if let Some(top_n) = bad_top_n {
        return Err(BtError::InvalidParam(format!(
            "top_n 须 >= 1，收到: {top_n}"
        )));
    }

    // 2. 装配（Exchange::new 的费用 / 阈值校验同样先于数据加载）
    let strategy: Box<dyn Strategy> = match params.strategy {
        StrategySpec::TopkDropout {
            top_n,
            drop_n,
            only_tradable,
            forbid_st,
        } => Box::new(
            TopkDropoutStrategy::new(top_n, drop_n)
                .with_only_tradable(only_tradable)
                .with_forbid_st(forbid_st),
        ),
        StrategySpec::Custom(s) => s,
        StrategySpec::Topk { top_n, forbid_st } => {
            Box::new(TopkStrategy::new(top_n).with_forbid_st(forbid_st))
        }
    };
    let exchange = Exchange::with_deal_price(
        params.exchange.deal_price,
        params.exchange.open_cost,
        params.exchange.close_cost,
        params.exchange.min_cost,
        params.exchange.fixed_slippage,
        params.exchange.min_slippage_ratio,
        params.exchange.volume_threshold,
        params.exchange.limit_threshold,
    )?;

    // 3. wap 与 deal_price 匹配校验（先于 Paths 数据加载 fail fast；
    //    Shared 数据的路径存在性校验无意义，跳过——wap 窗口一致性由装配期校验拦截）
    let wap_window = match params.exchange.deal_price {
        DealPrice::Wap { window, .. } => Some(window),
        _ => None,
    };
    if let DataSource::Paths(paths) = &params.data {
        if wap_window.is_some() && paths.wap.is_none() {
            return Err(BtError::InvalidParam(format!(
                "deal_price={} 需要 wap 时段数据，请设置 DataPaths.wap",
                params.exchange.deal_price
            )));
        }
        if paths.wap.is_some() && wap_window.is_none() {
            log::warn!(
                "DataPaths.wap 已提供但 deal_price={} 未使用 vwapN/twapN，忽略",
                params.exchange.deal_price
            );
        }
    }

    // 4. 数据 -> 运行 -> 报告
    let data = match params.data {
        DataSource::Paths(paths) => {
            let mut data = BTData::new().load_stock_bar(&paths.stock_bar)?;
            if let (Some(path), Some(window)) = (&paths.wap, wap_window) {
                data = data.load_wap(path, window)?;
            }
            data.load_benchmark(&paths.benchmark)?.build()?
        }
        // build 校验 stock_bar 存在（Backtest::new 的不变量），对共享数据同样生效
        DataSource::Shared(data) => data.build()?,
    };
    let account = Account::new(params.initial_cash);
    let mut backtest = Backtest::new(data, account, exchange, strategy)?.with_progress(params.progress);
    let result = backtest.run(signal, &params.start_date, &params.end_date)?;
    let report = result.gen_report(
        params.benchmark_name.as_str(),
        params.excess_method.as_str(),
    )?;
    Ok(BtOutput { result, report })
}

/// 便捷入口：等价于先 `load_signal(signal_path)` 再 [`run`]。
pub fn run_from_signal_file(params: BtParams, signal_path: &str) -> Result<BtOutput> {
    let signal = crate::signal::load_signal(signal_path)?;
    run(params, &signal)
}

/// 便捷构造：`BTreeMap<日期, (instrument, score) 列表>` -> `Signal`
/// （`SignalDay::from_pairs` 逐日校验，口径同 `load_signal`）。
pub fn signal_from_pairs(
    days: BTreeMap<NaiveDate, Vec<(String, f64)>>,
) -> Result<Signal> {
    let mut out = BTreeMap::new();
    for (date, pairs) in days {
        out.insert(date, SignalDay::from_pairs(pairs)?);
    }
    Ok(Signal::from_days(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BtParams {
        BtParams {
            data: DataSource::Paths(DataPaths {
                stock_bar: "x.csv".into(),
                benchmark: "b.csv".into(),
                wap: None,
            }),
            start_date: "2026-01-01".into(),
            end_date: "2026-06-01".into(),
            initial_cash: 1_000_000.0,
            strategy: StrategySpec::topk_dropout(10, 5),
            exchange: ExchangeParams::default(),
            benchmark_name: BenchmarkName::Zz1000,
            excess_method: ExcessMethod::Arithmetic,
            progress: false,
        }
    }

    #[test]
    fn nonpositive_cash_errors_before_data_load() {
        for bad in [0.0, -1.0, f64::NAN] {
            let mut p = params();
            p.initial_cash = bad;
            let err = run(p, &Signal::from_days(BTreeMap::new())).unwrap_err();
            assert!(err.to_string().contains("initial_cash"), "{err}");
        }
    }

    #[test]
    fn zero_top_n_errors_before_data_load() {
        let mut p = params();
        p.strategy = StrategySpec::topk_dropout(0, 1);
        let err = run(p, &Signal::from_days(BTreeMap::new())).unwrap_err();
        assert!(err.to_string().contains("top_n"), "{err}");
    }
}
