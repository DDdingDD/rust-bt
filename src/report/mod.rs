//! Report（报告，架构 §4.9）：逐 bar 指标、衍生指标、export_data 与绘图。

mod html;

use std::fs::File;
use std::io::Write;

use chrono::NaiveDate;
use polars::prelude::*;

use crate::account::DailyRecord;
use crate::error::Result;
use crate::result::Col;
use crate::types::{BenchmarkName, ExcessMethod};

/// 年化交易日数（规范"指标定义"）。
const TRADING_DAYS_PER_YEAR: f64 = 252.0;

/// 衍生指标（由逐 bar 原始指标派生，不在 export_data 中输出）。
#[derive(Clone, Copy, Debug, Default)]
pub struct DerivedStats {
    /// 年化收益率（含成本）
    pub annualized_return: f64,
    /// 年化波动率（含成本，ddof = 0）
    pub annualized_volatility: f64,
    /// 夏普比率（含成本，无风险利率 0，ddof = 0）
    pub sharpe: f64,
    /// 最大回撤（含成本净值序列）
    pub max_drawdown: f64,
    /// 超额年化收益率（含成本口径超额日收益复利）
    pub excess_annualized_return: f64,
    /// 信息比率（含成本口径超额，ddof = 0）
    pub information_ratio: f64,
    /// 年化收益率（不含成本口径，V' = V + 累计费用 近似）
    pub annualized_return_without_cost: f64,
    /// 超额年化收益率（不含成本口径）
    pub excess_annualized_return_without_cost: f64,
    /// 信息比率（不含成本口径超额）
    pub information_ratio_without_cost: f64,
}

/// 回测报告：逐 bar 指标容器 + 衍生指标 + 绘图序列。
pub struct Report {
    /// export_data 逐 bar 表
    metrics: DataFrame,
    /// 衍生指标
    pub derived: DerivedStats,
    dates: Vec<NaiveDate>,
    /// 报告基准名称（仅用于 summary / plot 展示，不进入导出表）
    benchmark_name: BenchmarkName,
    /// 超额收益口径（仅用于 summary / plot 展示）
    excess_method: ExcessMethod,
    /// 累计净值（含成本，期初 1）
    cum_with_cost: Vec<f64>,
    /// 累计净值（不含成本口径，V' = V + 累计费用 近似）
    cum_without_cost: Vec<f64>,
    /// 基准累计净值（期初 1）
    cum_benchmark: Vec<f64>,
    /// 回撤序列（含成本净值，正值）
    drawdown: Vec<f64>,
    /// 回撤序列（不含成本净值，正值）
    drawdown_without: Vec<f64>,
    /// 累计超额净值（含成本口径超额复利）
    cum_excess: Vec<f64>,
    /// 累计超额净值（不含成本口径超额复利）
    cum_excess_without: Vec<f64>,
    /// 超额净值回撤（含成本口径，正值）
    excess_drawdown: Vec<f64>,
    /// 超额净值回撤（不含成本口径，正值）
    excess_drawdown_without: Vec<f64>,
    /// 双边换手率（日度）
    turnover: Vec<f64>,
}

impl Report {
    /// 由逐日账户记录 + 基准日收益构建（指标公式见规范"指标定义"）。
    pub(crate) fn build(
        daily: &[DailyRecord],
        bench: &[f64],
        dates: Vec<NaiveDate>,
        initial_cash: f64,
        method: ExcessMethod,
        benchmark_name: BenchmarkName,
    ) -> Self {
        let n = daily.len();
        debug_assert_eq!(n, bench.len());

        let account: Vec<f64> = daily.iter().map(|d| d.account).collect();
        let value: Vec<f64> = daily.iter().map(|d| d.value).collect();
        let cash: Vec<f64> = daily.iter().map(|d| d.cash).collect();

        // 首日口径：r_0 = 0（首日盈亏不进入收益率与净值序列，仅体现在 account 列）
        let mut ret = vec![0.0f64; n];
        for t in 1..n {
            ret[t] = account[t] / account[t - 1] - 1.0;
        }
        // 分母：前一交易日总资产（首个有成交的交易日用期初资金）
        let denom = |t: usize| {
            if t == 0 {
                initial_cash
            } else {
                account[t - 1]
            }
        };
        // 双边换手率 = 当日双边成交金额（含滑点口径）/ 分母；无成交日记 0
        let turnover: Vec<f64> = daily
            .iter()
            .enumerate()
            .map(|(t, d)| {
                if d.turnover_amount == 0.0 {
                    0.0
                } else {
                    d.turnover_amount / denom(t)
                }
            })
            .collect();
        // 当日费用率；无成交日记 0
        let cost: Vec<f64> = daily
            .iter()
            .enumerate()
            .map(|(t, d)| {
                if d.cost == 0.0 {
                    0.0
                } else {
                    d.cost / denom(t)
                }
            })
            .collect();
        let mut total_turnover = Vec::with_capacity(n);
        let mut total_cost = Vec::with_capacity(n);
        let mut acc_t = 0.0;
        let mut acc_c = 0.0;
        for d in daily {
            acc_t += d.turnover_amount;
            acc_c += d.cost;
            total_turnover.push(acc_t);
            total_cost.push(acc_c);
        }

        // 不含成本口径（近似）：V'_t = V_t + 累计费用
        let v_prime: Vec<f64> = account
            .iter()
            .zip(&total_cost)
            .map(|(a, c)| a + c)
            .collect();
        let mut ret_without = vec![0.0f64; n];
        for t in 1..n {
            ret_without[t] = v_prime[t] / v_prime[t - 1] - 1.0;
        }

        // 超额日收益（含 / 不含成本各一条）
        let excess_with: Vec<f64> = ret
            .iter()
            .zip(bench)
            .map(|(r, b)| method.excess(*r, *b))
            .collect();
        let excess_without: Vec<f64> = ret_without
            .iter()
            .zip(bench)
            .map(|(r, b)| method.excess(*r, *b))
            .collect();

        // 绘图序列
        let cum_with_cost = cumprod(&ret);
        let cum_without_cost = cumprod(&ret_without);
        let cum_benchmark = cumprod(bench);
        let cum_excess = cumprod(&excess_with);
        let cum_excess_without = cumprod(&excess_without);
        let drawdown = drawdown_of(&cum_with_cost);
        let drawdown_without = drawdown_of(&cum_without_cost);
        let excess_drawdown = drawdown_of(&cum_excess);
        let excess_drawdown_without = drawdown_of(&cum_excess_without);

        let derived = DerivedStats {
            annualized_return: annualized(&ret),
            annualized_volatility: annualized_volatility(&ret),
            sharpe: sharpe(&ret),
            max_drawdown: drawdown.iter().copied().fold(0.0, f64::max),
            excess_annualized_return: annualized(&excess_with),
            information_ratio: info_ratio(&excess_with),
            annualized_return_without_cost: annualized(&ret_without),
            excess_annualized_return_without_cost: annualized(&excess_without),
            information_ratio_without_cost: info_ratio(&excess_without),
        };

        let datetime: Vec<String> = dates.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect();
        let metrics = DataFrame::new(vec![
            Series::new("datetime".into(), datetime).into(),
            Series::new("account".into(), account).into(),
            Series::new("return".into(), ret).into(),
            Series::new("total_turnover".into(), total_turnover).into(),
            Series::new("turnover".into(), turnover.clone()).into(),
            Series::new("total_cost".into(), total_cost).into(),
            Series::new("cost".into(), cost).into(),
            Series::new("value".into(), value).into(),
            Series::new("cash".into(), cash).into(),
            Series::new("benchmark".into(), bench.to_vec()).into(),
        ])
        .expect("metrics 各列等长");

        Self {
            metrics,
            derived,
            dates,
            benchmark_name,
            excess_method: method,
            cum_with_cost,
            cum_without_cost,
            cum_benchmark,
            drawdown,
            drawdown_without,
            cum_excess,
            cum_excess_without,
            excess_drawdown,
            excess_drawdown_without,
            turnover,
        }
    }

    // ---- 序列只读访问器（嵌入方程序化消费；语义见各字段注释） ----

    /// 逐 bar 交易日（与各序列等长）。
    pub fn dates(&self) -> &[NaiveDate] {
        &self.dates
    }
    /// export_data 逐 bar 指标表（account/return/turnover/cost/benchmark 等列）。
    pub fn metrics(&self) -> &DataFrame {
        &self.metrics
    }
    /// 累计净值（含成本，期初 1）。
    pub fn cum_with_cost(&self) -> &[f64] {
        &self.cum_with_cost
    }
    /// 累计净值（不含成本口径，V' = V + 累计费用 近似，期初 1）。
    pub fn cum_without_cost(&self) -> &[f64] {
        &self.cum_without_cost
    }
    /// 基准累计净值（期初 1）。
    pub fn cum_benchmark(&self) -> &[f64] {
        &self.cum_benchmark
    }
    /// 回撤序列（含成本净值，正值）。
    pub fn drawdown(&self) -> &[f64] {
        &self.drawdown
    }
    /// 回撤序列（不含成本净值，正值）。
    pub fn drawdown_without(&self) -> &[f64] {
        &self.drawdown_without
    }
    /// 累计超额净值（含成本口径超额复利）。
    pub fn cum_excess(&self) -> &[f64] {
        &self.cum_excess
    }
    /// 累计超额净值（不含成本口径超额复利）。
    pub fn cum_excess_without(&self) -> &[f64] {
        &self.cum_excess_without
    }
    /// 超额净值回撤（含成本口径，正值）。
    pub fn excess_drawdown(&self) -> &[f64] {
        &self.excess_drawdown
    }
    /// 超额净值回撤（不含成本口径，正值）。
    pub fn excess_drawdown_without(&self) -> &[f64] {
        &self.excess_drawdown_without
    }
    /// 双边换手率（日度）。
    pub fn turnover(&self) -> &[f64] {
        &self.turnover
    }

    /// 导出逐 bar 原始指标（规范"指标定义--export_data 输出"）。
    pub fn export_data(&self, path: &str) -> Result<()> {
        let mut df = self.metrics.clone();
        let file = File::create(path)?;
        CsvWriter::new(file).finish(&mut df)?;
        Ok(())
    }

    /// 绘制交互式报告：顶部衍生指标表 + 7 面板图（累计收益 / 两口径回撤 /
    /// 累计超额 / 换手率 / 两口径超额回撤），输出自包含 HTML（plotly.js 走 CDN）到指定路径
    /// （如 report_plot.html；X 轴为交易日，首点补 T0 基准）。
    pub fn plot(&self, path: &str) -> Result<()> {
        if self.dates.is_empty() {
            return Err(crate::error::BtError::Validation("无回测数据，无法绘图".into()));
        }
        let curves = html::ReportCurves {
            dates: &self.dates,
            cum_bench: &self.cum_benchmark,
            cum_wo_cost: &self.cum_without_cost,
            cum_w_cost: &self.cum_with_cost,
            mdd_wo_cost: &self.drawdown_without,
            mdd_w_cost: &self.drawdown,
            cum_ex_wo_cost: &self.cum_excess_without,
            cum_ex_w_cost: &self.cum_excess,
            turnover: &self.turnover,
            ex_mdd_w_cost: &self.excess_drawdown,
            ex_mdd_wo_cost: &self.excess_drawdown_without,
        };
        let mut file = File::create(path)?;
        file.write_all(html::render_html(&curves, &self.derived).as_bytes())?;
        Ok(())
    }

    /// 简报：关键指标文本（嵌入方日志 / 终端输出用；完整指标见 `derived` 与序列访问器）。
    pub fn summary(&self) -> String {
        let d = &self.derived;
        let (first, last) = match (self.dates.first(), self.dates.last()) {
            (Some(f), Some(l)) => (
                f.format("%Y-%m-%d").to_string(),
                l.format("%Y-%m-%d").to_string(),
            ),
            _ => ("-".into(), "-".into()),
        };
        let period_ret = self.cum_with_cost.last().unwrap_or(&1.0) - 1.0;
        let period_ret_wo = self.cum_without_cost.last().unwrap_or(&1.0) - 1.0;
        let avg_turnover = if self.turnover.is_empty() {
            0.0
        } else {
            self.turnover.iter().sum::<f64>() / self.turnover.len() as f64
        };

        format!(
            "回测区间: {first} ~ {last}（{} 个交易日，基准 {} / {}）\n\
             区间收益率:  {}（含成本）/ {}（不含成本）\n\
             年化收益率:  {}（含成本）/ {}（不含成本）\n\
             年化波动率:  {}\n\
             夏普比率:    {}\n\
             最大回撤:    {}\n\
             超额年化:    {}（含成本）/ {}（不含成本）\n\
             信息比率:    {}（含成本）/ {}（不含成本）\n\
             平均日换手率: {}",
            self.dates.len(),
            self.benchmark_name.as_str(),
            self.excess_method.as_str(),
            fmt_pct(period_ret),
            fmt_pct(period_ret_wo),
            fmt_pct(d.annualized_return),
            fmt_pct(d.annualized_return_without_cost),
            fmt_pct(d.annualized_volatility),
            fmt_ratio(d.sharpe),
            fmt_pct(d.max_drawdown),
            fmt_pct(d.excess_annualized_return),
            fmt_pct(d.excess_annualized_return_without_cost),
            fmt_ratio(d.information_ratio),
            fmt_ratio(d.information_ratio_without_cost),
            fmt_pct(avg_turnover),
        )
    }
}

fn fmt_pct(v: f64) -> String {
    format!("{:>8.2}%", v * 100.0)
}

fn fmt_ratio(v: f64) -> String {
    format!("{:>8.2}", v)
}

/// 导出 CSV（hist_position / trades 共用）。
pub(crate) fn write_csv(path: &str, cols: Vec<(&str, Col)>) -> Result<()> {
    let columns: Vec<Column> = cols
        .into_iter()
        .map(|(name, col)| -> Column {
            match col {
                Col::Str(v) => Series::new(name.into(), v).into(),
                Col::F64(v) => Series::new(name.into(), v).into(),
                Col::U32(v) => Series::new(name.into(), v).into(),
            }
        })
        .collect();
    let mut df = DataFrame::new(columns)?;
    let file = File::create(path)?;
    CsvWriter::new(file).finish(&mut df)?;
    Ok(())
}

/// 累计净值：cumprod(1 + r)，期初为 1。
fn cumprod(r: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(r.len());
    let mut v = 1.0;
    for x in r {
        v *= 1.0 + x;
        out.push(v);
    }
    out
}

/// 回撤序列：1 − 净值_t / max_{s≤t} 净值_s。
fn drawdown_of(cum: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(cum.len());
    let mut peak = f64::MIN;
    for v in cum {
        peak = peak.max(*v);
        out.push(if peak > 0.0 { 1.0 - v / peak } else { 0.0 });
    }
    out
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// 总体标准差（ddof = 0）。
fn std_dev(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let m = mean(xs);
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64).sqrt()
}

/// 年化收益率：(∏(1 + r_t))^(N/n) − 1；n 为区间交易日数（含首日）。
fn annualized(r: &[f64]) -> f64 {
    if r.is_empty() {
        return 0.0;
    }
    let cum: f64 = r.iter().map(|x| 1.0 + x).product();
    cum.powf(TRADING_DAYS_PER_YEAR / r.len() as f64) - 1.0
}

fn annualized_volatility(r: &[f64]) -> f64 {
    std_dev(r) * TRADING_DAYS_PER_YEAR.sqrt()
}

/// 夏普比率：mean / std × √N（无风险利率取 0，ddof = 0）。
fn sharpe(r: &[f64]) -> f64 {
    let s = std_dev(r);
    if s == 0.0 {
        0.0
    } else {
        mean(r) / s * TRADING_DAYS_PER_YEAR.sqrt()
    }
}

/// 信息比率：mean(excess) / std(excess) × √N（ddof = 0）。
fn info_ratio(excess: &[f64]) -> f64 {
    let s = std_dev(excess);
    if s == 0.0 {
        0.0
    } else {
        mean(excess) / s * TRADING_DAYS_PER_YEAR.sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn fake_daily() -> Vec<DailyRecord> {
        // 5 个交易日：T0 现金 100 万，T1 买入并稳定在 100 万（零成本），后续无交易
        let mut out = Vec::new();
        for i in 0..5 {
            let account = 1_000_000.0;
            out.push(DailyRecord {
                day: i,
                account,
                value: account,
                cash: 0.0,
                turnover_amount: if i == 1 { 1_000_000.0 } else { 0.0 },
                cost: 0.0,
            });
        }
        out
    }

    fn dates() -> Vec<NaiveDate> {
        (1..=5)
            .map(|d| NaiveDate::from_ymd_opt(2026, 1, d).unwrap())
            .collect()
    }

    #[test]
    fn summary_contains_key_metrics() {
        let daily = fake_daily();
        let bench = vec![0.0f64; daily.len()];
        let report = Report::build(
            &daily,
            &bench,
            dates(),
            1_000_000.0,
            ExcessMethod::Arithmetic,
            BenchmarkName::Zz1000,
        );

        let s = report.summary();
        assert!(s.contains("2026-01-01"), "{s}");
        assert!(s.contains("2026-01-05"), "{s}");
        assert!(s.contains("zz1000"), "{s}");
        assert!(s.contains("arithmetic"), "{s}");
        assert!(s.contains("年化收益率"), "{s}");
        assert!(s.contains("最大回撤"), "{s}");
        assert!(s.contains("信息比率"), "{s}");
        assert!(s.contains("平均日换手率"), "{s}");
    }
}
