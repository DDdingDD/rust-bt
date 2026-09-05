//! YAML 回测配置（供 `bt <config.yml>` CLI 使用）。
//!
//! 必填：信号路径（`signal`）、行情数据路径（`data.*`）与回测区间（`period.*`）；
//! 其余字段均可省略，默认值对齐 `examples/run_backtest.rs` 的取值。

use anyhow::{anyhow, Context};
use serde::Deserialize;

use crate::types::{BenchmarkName, DealPrice, ExcessMethod};

/// 顶层配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BtConfig {
    /// 信号 CSV 路径（pred.csv，必填）。
    pub signal: Option<String>,
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
        let text =
            std::fs::read_to_string(path).with_context(|| format!("读取配置文件失败: {path}"))?;
        let mut cfg: BtConfig =
            serde_yaml::from_str(&text).with_context(|| format!("解析 YAML 配置失败: {path}"))?;
        // 目录发现先于校验：data.dir 解析回填具体文件路径后，
        // 下游（check_shareable_data / to_params / CLI）看到的与显式配置无异
        cfg.resolve_data_dir()?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// `data.dir` 目录发现：显式路径优先，dir 只补缺失的 stock_bar / benchmark；
    /// wap 按需发现——仅 `deal_price` 为 vwapN/twapN 时查找（目录里的 wap 文件
    /// 对非时段价回测无影响，也不触发 validate 的 wap/deal_price 匹配报错）。
    fn resolve_data_dir(&mut self) -> anyhow::Result<()> {
        let Some(dir) = self.data.dir.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(());
        };
        let dir = dir.to_owned();
        if !std::path::Path::new(&dir).is_dir() {
            return Err(anyhow!("data.dir 不存在或不是目录: {dir}"));
        }
        let fill = |field: &str, stem: &str, slot: &mut Option<String>| -> anyhow::Result<()> {
            if slot.as_deref().is_some_and(|s| !s.is_empty()) {
                return Ok(()); // 显式路径优先
            }
            match crate::data::find_data_file(&dir, stem) {
                Some(p) => {
                    *slot = Some(p);
                    Ok(())
                }
                None => Err(anyhow!(
                    "data.dir 目录 {dir} 中未找到 {stem} 数据文件（{stem}.parquet / {stem}.pq / {stem}.csv），请显式配置 {field}"
                )),
            }
        };
        fill("data.stock_bar", "stock_bar", &mut self.data.stock_bar)?;
        fill("data.benchmark", "benchmark", &mut self.data.benchmark)?;
        let need_wap = matches!(
            DealPrice::parse(&self.exchange.deal_price),
            Ok(DealPrice::Wap { .. })
        );
        if need_wap {
            fill("data.wap", "wap", &mut self.data.wap)?;
        }
        Ok(())
    }

    fn validate(&self) -> anyhow::Result<()> {
        for (key, val) in [
            ("signal", &self.signal),
            ("data.stock_bar", &self.data.stock_bar),
            ("data.benchmark", &self.data.benchmark),
            ("period.start_date", &self.period.start_date),
            ("period.end_date", &self.period.end_date),
        ] {
            if val.as_deref().is_none_or(str::is_empty) {
                let hint = if key.starts_with("data.") {
                    "（或配置 data.dir 目录自动发现）"
                } else {
                    ""
                };
                return Err(anyhow!("配置缺少必填字段: {key}{hint}"));
            }
        }
        // 数值与枚举参数在加载期统一校验：避免加载数百 MB 行情后（甚至跑完整个
        // 回测后）才因参数拼写 / 越界报错
        let cash = self.account.initial_cash;
        if !cash.is_finite() || cash <= 0.0 {
            return Err(anyhow!("account.initial_cash 须为正数，收到: {cash}"));
        }
        if !matches!(self.strategy.name.as_str(), "topk_dropout" | "topk") {
            return Err(anyhow!(
                "未知策略: {}（当前支持 topk_dropout / topk）",
                self.strategy.name
            ));
        }
        if self.strategy.top_n < 1 {
            return Err(anyhow!(
                "strategy.top_n 须 >= 1，收到: {}",
                self.strategy.top_n
            ));
        }
        let deal_price = DealPrice::parse(&self.exchange.deal_price)
            .map_err(|e| anyhow!("exchange.deal_price 非法: {e}"))?;
        // deal_price 为 vwapN/twapN 时必须提供 wap 数据路径；反之提供了 wap 路径
        // 而 deal_price 未用时段价（多为拼写失误）也报错，防止静默忽略
        let wap_path = self.data.wap.as_deref().is_some_and(|s| !s.is_empty());
        match deal_price {
            DealPrice::Wap { .. } => {
                if !wap_path {
                    return Err(anyhow!(
                        "exchange.deal_price = {} 需要 data.wap（wap 时段数据路径）",
                        self.exchange.deal_price
                    ));
                }
            }
            _ => {
                if wap_path {
                    return Err(anyhow!(
                        "data.wap 已提供但 exchange.deal_price = {} 未使用 vwapN/twapN（时段价）",
                        self.exchange.deal_price
                    ));
                }
            }
        }
        if BenchmarkName::from_name(&self.report.benchmark).is_none() {
            return Err(anyhow!(
                "report.benchmark 未知基准名称: {}（不在映射表）",
                self.report.benchmark
            ));
        }
        ExcessMethod::parse(&self.report.excess_method)
            .map_err(|e| anyhow!("report.excess_method 非法: {e}"))?;
        Ok(())
    }

    /// 多配置共享数据前置校验（CLI 批量运行 `bt a.yml b.yml ...`，决策 D15）：
    /// 所有配置的数据路径须一致（stock_bar / benchmark 全部一致；wap 在提供它的
    /// 配置间一致），且 wap 模式配置的 deal_price 时段须一致（共享数据只含一个
    /// wap 时段）。`configs` 为 (配置文件路径, 配置) 对，报错信息据此指出冲突来源。
    pub fn check_shareable_data(configs: &[(&str, &BtConfig)]) -> anyhow::Result<()> {
        let Some((first_path, first)) = configs.first() else {
            return Ok(());
        };
        for (path, cfg) in &configs[1..] {
            for (key, a, b) in [
                ("data.stock_bar", first.stock_bar(), cfg.stock_bar()),
                (
                    "data.benchmark",
                    first.benchmark_data(),
                    cfg.benchmark_data(),
                ),
            ] {
                if a != b {
                    anyhow::bail!(
                        "多配置共享数据要求 {key} 一致：{first_path} 为 \"{a}\"，{path} 为 \"{b}\""
                    );
                }
            }
        }
        // wap：提供 data.wap 的配置间路径一致；wap 模式配置间时段一致
        let mut wap_src: Option<(&str, &str)> = None;
        let mut window_src: Option<(&str, u8)> = None;
        for (path, cfg) in configs {
            if let Some(w) = cfg.data.wap.as_deref().filter(|s| !s.is_empty()) {
                match wap_src {
                    Some((p0, w0)) if w0 != w => anyhow::bail!(
                        "多配置共享数据要求 data.wap 一致：{p0} 为 \"{w0}\"，{path} 为 \"{w}\""
                    ),
                    Some(_) => {}
                    None => wap_src = Some((path, w)),
                }
            }
            if let DealPrice::Wap { window, .. } = DealPrice::parse(&cfg.exchange.deal_price)
                .map_err(|e| anyhow!("exchange.deal_price 非法: {e}"))?
            {
                match window_src {
                    Some((p0, w0)) if w0 != window => anyhow::bail!(
                        "多配置共享数据要求 wap 时段一致：{p0} 为 wap_{w0}，{path} 为 wap_{window}"
                    ),
                    Some(_) => {}
                    None => window_src = Some((path, window)),
                }
            }
        }

        // 输出路径：多配置时各配置的四类产物不能写入同一文件，防止静默覆盖
        if configs.len() > 1 {
            let mut seen: std::collections::HashMap<
                (String, String, String, String, String),
                &str,
            > = std::collections::HashMap::new();
            for (path, cfg) in configs {
                let key = (
                    cfg.output.dir.clone(),
                    cfg.output.hist_position.clone(),
                    cfg.output.trades.clone(),
                    cfg.output.report_data.clone(),
                    cfg.output.report_plot.clone(),
                );
                if let Some(prev) = seen.get(&key) {
                    anyhow::bail!(
                        "多配置批量运行时输出产物冲突：{prev} 与 {path} 使用相同的 output.dir=\"{}\" 与文件名，后运行的配置会覆盖先运行的产物",
                        cfg.output.dir
                    );
                }
                seen.insert(key, path);
            }
        }
        Ok(())
    }

    // ---- 必填字段访问器（load 已校验，此处直接 unwrap） ----

    pub fn signal(&self) -> &str {
        self.signal.as_deref().expect("load 已校验")
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

    /// 转为嵌入 API 参数（CLI 组装用）。字符串枚举在此解析为类型化枚举；
    /// `load`/`validate` 已做过同等校验，错误分支实际不可达，仍带上下文透出。
    pub fn to_params(&self) -> anyhow::Result<crate::api::BtParams> {
        use crate::api::{BtParams, DataPaths, DataSource, ExchangeParams, StrategySpec};
        use crate::types::{BenchmarkName, DealPrice, ExcessMethod};

        let strategy = match self.strategy.name.as_str() {
            "topk_dropout" => StrategySpec::TopkDropout {
                top_n: self.strategy.top_n,
                drop_n: self.strategy.drop_n,
                only_tradable: self.strategy.only_tradable,
                forbid_st: self.strategy.forbid_st,
            },
            "topk" => StrategySpec::Topk {
                top_n: self.strategy.top_n,
                forbid_st: self.strategy.forbid_st,
            },
            other => anyhow::bail!("未知策略: {other}"),
        };
        Ok(BtParams {
            data: DataSource::Paths(DataPaths {
                stock_bar: self.stock_bar().to_owned(),
                benchmark: self.benchmark_data().to_owned(),
                wap: self.data.wap.clone(),
            }),
            start_date: self.start_date().to_owned(),
            end_date: self.end_date().to_owned(),
            initial_cash: self.account.initial_cash,
            strategy,
            exchange: ExchangeParams {
                deal_price: DealPrice::parse(&self.exchange.deal_price)
                    .map_err(|e| anyhow!("exchange.deal_price 非法: {e}"))?,
                open_cost: self.exchange.open_cost,
                close_cost: self.exchange.close_cost,
                min_cost: self.exchange.min_cost,
                fixed_slippage: self.exchange.fixed_slippage,
                min_slippage_ratio: self.exchange.min_slippage_ratio,
                volume_threshold: self.exchange.volume_threshold,
                limit_threshold: self.exchange.limit_threshold,
            },
            benchmark_name: BenchmarkName::from_name(&self.report.benchmark).ok_or_else(|| {
                anyhow!("report.benchmark 未知基准名称: {}", self.report.benchmark)
            })?,
            excess_method: ExcessMethod::parse(&self.report.excess_method)
                .map_err(|e| anyhow!("report.excess_method 非法: {e}"))?,
            progress: self.progress,
        })
    }
}

impl Default for BtConfig {
    fn default() -> Self {
        Self {
            signal: None,
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

/// 行情数据文件路径（stock_bar 与 benchmark 必填；wap 在 deal_price 为
/// vwapN/twapN 时必填，见 `ExchangeConfig::deal_price`）。
///
/// 也可改为提供 `dir` 目录自动发现：按文件名主干 stock_bar / benchmark /
/// wap 匹配，扩展名优先级 parquet > pq > csv；显式路径优先于目录发现
/// （可只显式覆盖其中一项），wap 仅在 deal_price 为时段价时查找。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DataConfig {
    /// 数据目录：自动发现 stock_bar / benchmark（按需 wap）数据文件。
    pub dir: Option<String>,
    /// 股票日行情 CSV / parquet（stock_bar）。
    pub stock_bar: Option<String>,
    /// 基准收益 CSV / parquet（benchmark）。
    pub benchmark: Option<String>,
    /// wap 时段数据（wap.csv / wap.parquet，deal_price 为 vwapN/twapN 时必填）。
    pub wap: Option<String>,
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
    /// 策略名（"topk_dropout" / "topk"）。
    pub name: String,
    /// 目标持仓只数。
    pub top_n: usize,
    /// 每个调仓日计划卖出的只数（仅 topk_dropout 使用）。
    pub drop_n: usize,
    /// 卖出候选是否限定当日可交易股票（仅 topk_dropout 使用）。
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
    /// 成交价：open / close / vwap / vwapN / twapN（N = 1..=11 时段价，需 data.wap；
    /// 时段表见 doc/specification.md "数据文件格式--wap 数据"）。
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
signal: "tmp_data/pred.csv"
data:
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
        assert!(err.contains("signal"), "报错应含字段名: {err}");
    }

    #[test]
    fn full_config_overrides_defaults() {
        let yaml = r#"
signal: "s.csv"
data:
  stock_bar: "b.csv"
  benchmark: "m.csv"
  wap: "w.parquet"
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
  deal_price: "vwap11"
  open_cost: 0.0002
  volume_threshold: null
  limit_threshold: null
report:
  benchmark: "hs300"
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
        assert_eq!(cfg.exchange.deal_price, "vwap11");
        assert_eq!(cfg.data.wap.as_deref(), Some("w.parquet"));
        assert_eq!(cfg.exchange.open_cost, 0.0002);
        // null 显式表示不限制；未写的字段仍取默认
        assert_eq!(cfg.exchange.volume_threshold, None);
        assert_eq!(cfg.exchange.limit_threshold, None);
        assert_eq!(cfg.exchange.close_cost, 0.00065);
        assert_eq!(cfg.report.benchmark, "hs300");
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

    #[test]
    fn topk_strategy_maps_to_spec() {
        let yaml =
            format!("{MINIMAL_YAML}strategy:\n  name: \"topk\"\n  top_n: 30\n  forbid_st: true\n");
        let cfg = parse(&yaml).unwrap();
        let params = cfg.to_params().unwrap();
        match params.strategy {
            crate::api::StrategySpec::Topk { top_n, forbid_st } => {
                assert_eq!(top_n, 30);
                assert!(forbid_st);
            }
            other => panic!("期望 StrategySpec::Topk，实际: {other:?}"),
        }
    }

    #[test]
    fn invalid_initial_cash_errors() {
        // 0 / 负数 / NaN 的期初资金会让报告收益率静默产生 NaN，加载期即报错
        for bad in ["0", "-1000000", ".nan"] {
            let yaml = format!("{MINIMAL_YAML}account:\n  initial_cash: {bad}\n");
            let err = parse(&yaml).unwrap_err().to_string();
            assert!(err.contains("initial_cash"), "报错应含字段名: {err}");
        }
    }

    #[test]
    fn top_n_zero_errors() {
        // top_n = 0 在策略构造时会 panic（库层 assert），配置层提前拦截为友好报错
        let yaml = format!("{MINIMAL_YAML}strategy:\n  top_n: 0\n");
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(err.contains("top_n"), "报错应含字段名: {err}");
    }

    #[test]
    fn invalid_enum_params_error_at_load() {
        // 拼写错误的枚举参数在加载期报错，而非数据加载后 / 回测结束后
        let yaml = format!("{MINIMAL_YAML}report:\n  benchmark: \"csi300\"\n");
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(err.contains("csi300"), "报错应含基准名: {err}");

        let yaml = format!("{MINIMAL_YAML}report:\n  excess_method: \"geo\"\n");
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(err.contains("excess_method"), "报错应含字段名: {err}");

        let yaml = format!("{MINIMAL_YAML}exchange:\n  deal_price: \"avg\"\n");
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(err.contains("deal_price"), "报错应含字段名: {err}");
    }

    #[test]
    fn wap_deal_price_requires_wap_path() {
        // vwapN / twapN 需要 data.wap
        let yaml = format!("{MINIMAL_YAML}exchange:\n  deal_price: \"vwap11\"\n");
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(err.contains("data.wap"), "vwap11 缺 wap 路径应报错: {err}");

        let yaml = format!("{MINIMAL_YAML}exchange:\n  deal_price: \"twap3\"\n");
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(err.contains("data.wap"), "twap3 缺 wap 路径应报错: {err}");
    }

    #[test]
    fn wap_path_only_with_wap_deal_price() {
        // data.wap 提供但 deal_price 未用时段价（多为拼写失误）也报错
        let yaml = MINIMAL_YAML.replace(
            "  benchmark: \"tmp_data/benchmark.csv\"",
            "  benchmark: \"tmp_data/benchmark.csv\"\n  wap: \"w.parquet\"",
        );
        let err = parse(&yaml).unwrap_err().to_string();
        assert!(
            err.contains("data.wap"),
            "wap 提供但 deal_price 非时段价应报错: {err}"
        );
    }

    #[test]
    fn shareable_data_check() {
        let a = parse(MINIMAL_YAML).unwrap();

        // 单配置运行时即使输出路径相同也不冲突
        BtConfig::check_shareable_data(&[("a.yml", &a)]).unwrap();

        // 多配置需区分输出路径
        let b = parse(&format!("{MINIMAL_YAML}strategy:\n  top_n: 30\n")).unwrap();
        let err = BtConfig::check_shareable_data(&[("a.yml", &a), ("b.yml", &b)])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("输出产物冲突") && err.contains("a.yml") && err.contains("b.yml"),
            "{err}"
        );

        let b_yaml = format!("{MINIMAL_YAML}output:\n  dir: \"output_b\"\n");
        let b_out = parse(&b_yaml).unwrap();
        BtConfig::check_shareable_data(&[("a.yml", &a), ("b.yml", &b_out)]).unwrap();

        // wap 配置间默认输出路径也冲突
        let wap_yaml = |window: u8| {
            MINIMAL_YAML.replace(
                "  benchmark: \"tmp_data/benchmark.csv\"",
                &format!(
                    "  benchmark: \"tmp_data/benchmark.csv\"\n  wap: \"w.parquet\"\nexchange:\n  deal_price: \"vwap{window}\""
                ),
            )
        };
        let w1 = parse(&wap_yaml(11)).unwrap();
        let w2_yaml = format!("{}output:\n  dir: \"output_w\"\n", wap_yaml(11));
        let w2 = parse(&w2_yaml).unwrap();
        BtConfig::check_shareable_data(&[("w1.yml", &w1), ("w2.yml", &w2)]).unwrap();

        // stock_bar 不一致 -> 报错并指出冲突配置
        let c = parse(&MINIMAL_YAML.replace("stock_bar.csv", "other.csv")).unwrap();
        let err = BtConfig::check_shareable_data(&[("a.yml", &a), ("c.yml", &c)])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("data.stock_bar") && err.contains("c.yml"),
            "{err}"
        );

        // wap 时段不一致 -> 报错
        let w3 = parse(&wap_yaml(3)).unwrap();
        let err = BtConfig::check_shareable_data(&[("w1.yml", &w1), ("w3.yml", &w3)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("wap 时段") && err.contains("w3.yml"), "{err}");

        // wap 与非 wap 配置混跑允许（非 wap 配置不提供 data.wap，单配置校验已保证），
        // 但输出路径仍须不同
        BtConfig::check_shareable_data(&[("a.yml", &a), ("w1.yml", &w2)]).unwrap();
    }

    /// 写入临时 YAML 文件后走真实 `BtConfig::load`（data.dir 解析只在 load 中发生）。
    fn load_yaml(yaml: &str) -> anyhow::Result<BtConfig> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.yml");
        std::fs::write(&path, yaml).unwrap();
        BtConfig::load(path.to_str().unwrap())
    }

    #[test]
    fn data_dir_auto_discovery() {
        let dir = tempfile::tempdir().unwrap();
        // 多格式并存时 parquet 优先；单引号 YAML 字符串避免 Windows 路径反斜杠转义
        for f in ["stock_bar.parquet", "stock_bar.csv", "benchmark.csv"] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        let yaml = format!(
            "signal: \"s.csv\"\ndata:\n  dir: '{}'\nperiod:\n  start_date: \"2026-01-01\"\n  end_date: \"2026-06-01\"\n",
            dir.path().display()
        );
        let cfg = load_yaml(&yaml).unwrap();
        assert!(
            cfg.stock_bar().ends_with("stock_bar.parquet"),
            "{}",
            cfg.stock_bar()
        );
        assert!(
            cfg.benchmark_data().ends_with("benchmark.csv"),
            "{}",
            cfg.benchmark_data()
        );
        assert_eq!(cfg.data.wap, None);
    }

    #[test]
    fn data_dir_explicit_path_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("benchmark.csv"), "x").unwrap();
        let yaml = format!(
            "signal: \"s.csv\"\ndata:\n  dir: '{}'\n  stock_bar: \"explicit/bar.csv\"\nperiod:\n  start_date: \"2026-01-01\"\n  end_date: \"2026-06-01\"\n",
            dir.path().display()
        );
        // 显式 stock_bar 优先（即使目录里没有 stock_bar 文件），benchmark 由 dir 补齐
        let cfg = load_yaml(&yaml).unwrap();
        assert_eq!(cfg.stock_bar(), "explicit/bar.csv");
        assert!(cfg.benchmark_data().ends_with("benchmark.csv"));
    }

    #[test]
    fn data_dir_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = format!(
            "signal: \"s.csv\"\ndata:\n  dir: '{}'\nperiod:\n  start_date: \"2026-01-01\"\n  end_date: \"2026-06-01\"\n",
            dir.path().display()
        );
        let err = load_yaml(&yaml).unwrap_err().to_string();
        assert!(err.contains("stock_bar"), "缺文件报错应含主干名: {err}");

        let err = load_yaml(&yaml.replace(dir.path().to_str().unwrap(), "no/such/dir"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("data.dir"), "目录无效应报 data.dir: {err}");
    }

    #[test]
    fn data_dir_wap_on_demand() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["stock_bar.csv", "benchmark.csv", "wap.csv"] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        let base = format!(
            "signal: \"s.csv\"\ndata:\n  dir: '{}'\nperiod:\n  start_date: \"2026-01-01\"\n  end_date: \"2026-06-01\"\n",
            dir.path().display()
        );
        // deal_price 非时段价：目录里的 wap.csv 被忽略，不触发 wap/deal_price 匹配报错
        let cfg = load_yaml(&base).unwrap();
        assert_eq!(cfg.data.wap, None);

        // deal_price 为时段价：wap 按需发现
        let cfg = load_yaml(&format!("{base}exchange:\n  deal_price: \"vwap3\"\n")).unwrap();
        assert!(cfg.data.wap.as_deref().unwrap().ends_with("wap.csv"));

        // 时段价但目录中无 wap 文件：解析期报错
        std::fs::remove_file(dir.path().join("wap.csv")).unwrap();
        let err = load_yaml(&format!("{base}exchange:\n  deal_price: \"vwap3\"\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("wap"), "{err}");
    }
}
