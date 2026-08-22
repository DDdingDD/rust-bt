//! Report（报告，架构 §4.9）：逐 bar 指标、衍生指标、export_data 与绘图。

pub mod plot;

use std::fs::File;

use chrono::NaiveDate;
use polars::prelude::*;

use crate::account::DailyRecord;
use crate::error::Result;
use crate::result::Col;
use crate::types::ExcessMethod;

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
    /// 累计净值（含成本，期初 1）
    cum_with_cost: Vec<f64>,
    /// 基准累计净值（期初 1）
    cum_benchmark: Vec<f64>,
    /// 回撤序列（含成本净值）
    drawdown: Vec<f64>,
    /// 累计超额净值（含成本口径超额复利）
    cum_excess: Vec<f64>,
}

impl Report {
    /// 由逐日账户记录 + 基准日收益构建（指标公式见规范"指标定义"）。
    pub(crate) fn build(
        daily: &[DailyRecord],
        bench: &[f64],
        dates: Vec<NaiveDate>,
        initial_cash: f64,
        method: ExcessMethod,
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
        let cum_benchmark = cumprod(bench);
        let cum_excess = cumprod(&excess_with);
        let drawdown = drawdown_of(&cum_with_cost);

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
            Series::new("turnover".into(), turnover).into(),
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
            cum_with_cost,
            cum_benchmark,
            drawdown,
            cum_excess,
        }
    }

    /// 导出逐 bar 原始指标（规范"指标定义--export_data 输出"）。
    pub fn export_data(&self, path: &str) -> Result<()> {
        let mut df = self.metrics.clone();
        let file = File::create(path)?;
        CsvWriter::new(file).finish(&mut df)?;
        Ok(())
    }

    /// 绘制净值 / 回撤 / 超额三条曲线，输出 report_plot.png（X 轴为交易日）。
    pub fn plot(&self) -> Result<()> {
        plot::plot_report(
            "report_plot.png",
            &self.dates,
            &self.cum_with_cost,
            &self.cum_benchmark,
            &self.drawdown,
            &self.cum_excess,
        )
    }
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
