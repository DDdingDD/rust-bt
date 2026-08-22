//! YAML 回测配置（供 `bt <config.yml>` CLI 使用）。
//!
//! 必填：数据路径（`data.*`）与回测区间（`period.*`）；其余字段均可省略，
//! 默认值对齐 `examples/run_backtest.rs` 的取值。

use anyhow::{anyhow, Context};
use serde::Deserialize;

/// 顶层配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BtConfig {
    pub data: DataConfig,
    pub period: PeriodConfig,
    pub account: AccountConfig,
    pub strategy: StrategyConfig,
    pub exchange: ExchangeConfig,
    pub report: ReportConfig,
    pub output: OutputConfig,
    /// 终端进度条开关（默认开启）。
    pub progress: bool,
}

impl BtConfig {
    /// 从 YAML 文件加载配置，并校验必填字段。
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取配置文件失败: {path}"))?;
        let cfg: BtConfig = serde_yaml::from_str(&text)
            .with_context(|| format!("解析 YAML 配置失败: {path}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        for (key, val) in [
            ("data.signal", &self.data.signal),
            ("data.stock_bar", &self.data.stock_bar),
            ("data.benchmark", &self.data.benchmark),
            ("period.start_date", &self.period.start_date),
            ("period.end_date", &self.period.end_date),
        ] {
            if val.as_deref().map_or(true, str::is_empty) {
                return Err(anyhow!("配置缺少必填字段: {key}"));
            }
        }
        if self.strategy.name != "topk_dropout" {
            return Err(anyhow!(
                "未知策略: {}（当前仅支持 topk_dropout）",
                self.strategy.name
            ));
        }
        Ok(())
    }

    // ---- 必填字段访问器（load 已校验，此处直接 unwrap） ----

    pub fn signal(&self) -> &str {
        self.data.signal.as_deref().expect("load 已校验")
    }
    pub fn stock_bar(&self) -> &str {
        self.data.stock_bar.as_deref().expect("load 已校验")
    }
    pub fn benchmark_data(&self) -> &str {
        self.data.benchmark.as_deref().expect("load 已校验")
    }
    pub fn start_date(&self) -> &str {
        self.period.start_date.as_deref().expect("load 已校验")
    }
    pub fn end_date(&self) -> &str {
        self.period.end_date.as_deref().expect("load 已校验")
    }
}

impl Default for BtConfig {
    fn default() -> Self {
        Self {
            data: DataConfig::default(),
            period: PeriodConfig::default(),
            account: AccountConfig::default(),
            strategy: StrategyConfig::default(),
            exchange: ExchangeConfig::default(),
            report: ReportConfig::default(),
            output: OutputConfig::default(),
            progress: true,
        }
    }
}

/// 数据文件路径（全部必填）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DataConfig {
    /// 信号 CSV（pred.csv）。
    pub signal: Option<String>,
    /// 股票日行情 CSV（stock_bar.csv）。
    pub stock_bar: Option<String>,
    /// 基准收益 CSV（benchmark.csv）。
    pub benchmark: Option<String>,
}

/// 回测区间：闭开区间 [start_date, end_date)，自动按交易日历对齐（全部必填）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PeriodConfig {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

/// 账户参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AccountConfig {
    /// 期初资金（默认 1000 万）。
    pub initial_cash: f64,
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            initial_cash: 10_000_000.0,
        }
    }
}

/// 策略参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StrategyConfig {
    /// 策略名（当前仅支持 "topk_dropout"）。
    pub name: String,
    /// 目标持仓只数。
    pub top_n: usize,
    /// 每个调仓日计划卖出的只数。
    pub drop_n: usize,
    /// 仅买入当日可交易（非停牌/涨跌停）的股票。
    pub only_tradable: bool,
    /// 禁止买入 ST 股。
    pub forbid_st: bool,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            name: "topk_dropout".to_string(),
            top_n: 100,
            drop_n: 100,
            only_tradable: false,
            forbid_st: false,
        }
    }
}

/// 交易所（撮合与成本）参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ExchangeConfig {
    /// 成交价：open / close / vwap。
    pub deal_price: String,
    /// 买入费率（万 1.5）。
    pub open_cost: f64,
    /// 卖出费率（佣金 + 印花税，万 6.5）。
    pub close_cost: f64,
    /// 单笔最低费用。
    pub min_cost: f64,
    /// 固定滑点。
    pub fixed_slippage: f64,
    /// 最小滑点比例。
    pub min_slippage_ratio: f64,
    /// 成交量限制比例（null 表示不限制）。
    pub volume_threshold: Option<f64>,
    /// 涨跌停判定阈值（null 表示不限制）。
    pub limit_threshold: Option<f64>,
}

impl Default for ExchangeConfig {
    fn default() -> Self {
        Self {
            deal_price: "open".to_string(),
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

/// 报告参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReportConfig {
    /// 基准名称。
    pub benchmark: String,
    /// 超额计算方法：arithmetic / geometric。
    pub excess_method: String,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            benchmark: "zz1000".to_string(),
            excess_method: "arithmetic".to_string(),
        }
    }
}

/// 输出产物路径。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// 输出目录（不存在时自动创建）。
    pub dir: String,
    pub hist_position: String,
    pub trades: String,
    pub report_data: String,
    pub report_plot: String,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            dir: "output".to_string(),
            hist_position: "hist_position.csv".to_string(),
            trades: "trades.csv".to_string(),
            report_data: "report_data.csv".to_string(),
            report_plot: "report_plot.html".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_YAML: &str = r#"
data:
  signal: "tmp_data/pred.csv"
  stock_bar: "tmp_data/stock_bar.csv"
  benchmark: "tmp_data/benchmark.csv"
period:
  start_date: "2026-01-01"
  end_date: "2026-06-01"
"#;

    fn parse(yaml: &str) -> anyhow::Result<BtConfig> {
        let cfg: BtConfig = serde_yaml::from_str(yaml)?;
        cfg.validate()?;
        Ok(cfg)
    }

    #[test]
    fn minimal_config_uses_defaults() {
        let cfg = parse(MINIMAL_YAML).unwrap();
        assert_eq!(cfg.signal(), "tmp_data/pred.csv");
        assert_eq!(cfg.start_date(), "2026-01-01");
        assert_eq!(cfg.account.initial_cash, 10_000_000.0);
        assert_eq!(cfg.strategy.name, "topk_dropout");
        assert_eq!(cfg.strategy.top_n, 100);
        assert_eq!(cfg.strategy.drop_n, 100);
        assert!(!cfg.strategy.only_tradable);
        assert!(!cfg.strategy.forbid_st);
        assert_eq!(cfg.exchange.deal_price, "open");
        assert_eq!(cfg.exchange.open_cost, 0.00015);
        assert_eq!(cfg.exchange.close_cost, 0.00065);
        assert_eq!(cfg.exchange.min_cost, 5.0);
        assert_eq!(cfg.exchange.fixed_slippage, 0.01);
        assert_eq!(cfg.exchange.min_slippage_ratio, 0.0014);
        assert_eq!(cfg.exchange.volume_threshold, Some(0.5));
        assert_eq!(cfg.exchange.limit_threshold, Some(0.0985));
        assert_eq!(cfg.report.benchmark, "zz1000");
        assert_eq!(cfg.report.excess_method, "arithmetic");
        assert_eq!(cfg.output.dir, "output");
        assert!(cfg.progress);
    }

    #[test]
    fn missing_required_field_errors_with_name() {
        let yaml = r#"
data:
  stock_bar: "a.csv"
  benchmark: "b.csv"
period:
  start_date: "2026-01-01"
  end_date: "2026-06-01"
"#;
        let err = parse(yaml).unwrap_err().to_string();
        assert!(err.contains("data.signal"), "报错应含字段名: {err}");
    }

    #[test]
    fn full_config_overrides_defaults() {
        let yaml = r#"
data:
  signal: "s.csv"
  stock_bar: "b.csv"
  benchmark: "m.csv"
period:
  start_date: "2026-01-01"
  end_date: "2026-03-01"
account:
  initial_cash: 5000000.0
strategy:
  top_n: 50
  drop_n: 5
  only_tradable: true
  forbid_st: true
exchange:
  deal_price: "vwap"
  open_cost: 0.0002
  volume_threshold: null
  limit_threshold: null
report:
  benchmark: "csi300"
  excess_method: "geometric"
output:
  dir: "out2"
  report_plot: "plot.html"
progress: false
"#;
        let cfg = parse(yaml).unwrap();
        assert_eq!(cfg.account.initial_cash, 5_000_000.0);
        assert_eq!(cfg.strategy.top_n, 50);
        assert_eq!(cfg.strategy.drop_n, 5);
        assert!(cfg.strategy.only_tradable);
        assert!(cfg.strategy.forbid_st);
        assert_eq!(cfg.exchange.deal_price, "vwap");
        assert_eq!(cfg.exchange.open_cost, 0.0002);
        // null 显式表示不限制；未写的字段仍取默认
        assert_eq!(cfg.exchange.volume_threshold, None);
        assert_eq!(cfg.exchange.limit_threshold, None);
        assert_eq!(cfg.exchange.close_cost, 0.00065);
        assert_eq!(cfg.report.benchmark, "csi300");
        assert_eq!(cfg.report.excess_method, "geometric");
        assert_eq!(cfg.output.dir, "out2");
        assert_eq!(cfg.output.report_plot, "plot.html");
        assert_eq!(cfg.output.trades, "trades.csv");
        assert!(!cfg.progress);
    }

    #[test]
    fn unknown_strategy_errors() {
        let yaml = MINIMAL_YAML.replace("period:", "strategy:\n  name: \"momentum\"\nperiod:");
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(err.contains("momentum"), "报错应含策略名: {err}");
    }
}
