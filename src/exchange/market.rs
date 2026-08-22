//! DailyMarketStore（架构 §4.7）：行情注入 Exchange 时按 deal_price 预计算
//! `limit_buy` / `limit_sell`，并提供按日 SoA 视图与 `TradableInfo` 构建
//! （Exchange 撮合与 Strategy 决策共用同一份当日数据，口径一致，架构 D4）。

use crate::data::stock_bar::StockBarStore;
use crate::exchange::rules;
use crate::types::{Code, DayIdx, DealPrice, StockTradable, TradableInfo};

/// 注入行情后的按日市场存储：原始行情 + 预计算列，按 (DayIdx, Code) 排序。
pub struct DailyMarketStore {
    bar: StockBarStore,
    /// deal_price 列价格（缺失 / NaN / ≤ 0 时为 NaN，当日该股不可交易）
    deal_price: Vec<f64>,
    limit_buy: Vec<bool>,
    limit_sell: Vec<bool>,
    /// 当日成交量上限：volume × volume_threshold；None -> +∞；无量 / 缺失 -> 0
    volume_cap: Vec<f64>,
    /// 停牌：paused = 1 或 close 缺失
    suspended: Vec<bool>,
    /// 估值可用：未停牌且 close > 0
    valid_close: Vec<bool>,
}

impl DailyMarketStore {
    /// 注入行情：按 deal_price 预计算可交易性相关列。
    pub fn build(
        bar: StockBarStore,
        deal_price_col: DealPrice,
        volume_threshold: Option<f64>,
        limit_threshold: Option<f64>,
    ) -> Self {
        let n = bar.codes.len();
        let price_col: &[f64] = match deal_price_col {
            DealPrice::Open => &bar.open,
            DealPrice::Close => &bar.close,
            DealPrice::Vwap => &bar.vwap,
        };

        let mut deal_price = Vec::with_capacity(n);
        let mut limit_buy = Vec::with_capacity(n);
        let mut limit_sell = Vec::with_capacity(n);
        let mut volume_cap = Vec::with_capacity(n);
        let mut suspended = Vec::with_capacity(n);
        let mut valid_close = Vec::with_capacity(n);

        let mut missing_limit_warn = 0usize;
        let threshold_ratio = limit_threshold.map(|t| t / 0.1);

        for i in 0..n {
            let paused = bar.paused[i];
            let close = bar.close[i];
            let susp = paused || close.is_nan();
            suspended.push(susp);
            valid_close.push(!susp && close > 0.0);

            // deal_price 列缺失 / NaN / <= 0 -> 不可交易（NaN 标记）
            let p = price_col[i];
            deal_price.push(if p.is_nan() || p <= 0.0 { f64::NAN } else { p });

            // 当日 volume 缺失或为 0 -> 可成交量 0（与 volume_threshold 是否设置无关）
            let v = bar.volume[i];
            let cap = if v.is_nan() || v <= 0.0 {
                0.0
            } else {
                match volume_threshold {
                    Some(t) => v * t,
                    None => f64::INFINITY,
                }
            };
            volume_cap.push(cap);

            // 涨跌停预计算（判定顺序：先停牌后涨跌停——停牌行 limit 预计算无意义，
            // 由撮合时的停牌检查先行拦截）
            let (lb, ls) = match threshold_ratio {
                None => (false, false), // None: 不做涨跌停限制（构建时已 warning）
                Some(r) => {
                    let pre = bar.pre_close[i];
                    let hl = bar.high_limit[i];
                    let ll = bar.low_limit[i];
                    if pre.is_nan() || pre <= 0.0 {
                        // pre_close 缺失（如上市首日）：不做涨跌停判定，不告警
                        (false, false)
                    } else if hl.is_nan() || ll.is_nan() || hl <= 0.0 || ll <= 0.0 {
                        // high_limit / low_limit 自身缺失：置 false 并 warning（聚合计数）
                        missing_limit_warn += 1;
                        (false, false)
                    } else if p.is_nan() || p <= 0.0 {
                        // deal_price 列无效：不可交易，limit 标记无意义
                        (false, false)
                    } else {
                        rules::limit_flags(pre, hl, ll, p, r)
                    }
                }
            };
            limit_buy.push(lb);
            limit_sell.push(ls);
        }
        if missing_limit_warn > 0 {
            log::warn!(
                "stock_bar: high_limit/low_limit 缺失或非法 {missing_limit_warn} 行，当日不做涨跌停判定"
            );
        }

        Self {
            bar,
            deal_price,
            limit_buy,
            limit_sell,
            volume_cap,
            suspended,
            valid_close,
        }
    }

    /// 行情全部股票 code 集（信号"instrument 无行情"校验用）。
    pub fn code_set(&self) -> &std::collections::HashSet<Code> {
        &self.bar.code_set
    }

    /// 取当日 SoA 视图。
    pub fn day_view(&self, day: DayIdx) -> DayView<'_> {
        let range = self.bar.day_range(day);
        DayView {
            codes: &self.bar.codes[range.clone()],
            deal_price: &self.deal_price[range.clone()],
            limit_buy: &self.limit_buy[range.clone()],
            limit_sell: &self.limit_sell[range.clone()],
            volume_cap: &self.volume_cap[range.clone()],
            suspended: &self.suspended[range.clone()],
            valid_close: &self.valid_close[range.clone()],
            close: &self.bar.close[range.clone()],
            factor: &self.bar.factor[range.clone()],
            is_st: &self.bar.is_st[range],
        }
    }
}

/// 单日市场视图：撮合与账户估值共用。
#[derive(Clone, Copy)]
pub struct DayView<'a> {
    pub codes: &'a [Code],
    pub deal_price: &'a [f64],
    pub limit_buy: &'a [bool],
    pub limit_sell: &'a [bool],
    pub volume_cap: &'a [f64],
    pub suspended: &'a [bool],
    pub valid_close: &'a [bool],
    pub close: &'a [f64],
    pub factor: &'a [f64],
    pub is_st: &'a [bool],
}

impl<'a> DayView<'a> {
    fn idx(&self, code: Code) -> Option<usize> {
        self.codes.binary_search(&code).ok()
    }

    /// 单只股票当日行情行（撮合用）；`None` = 当日无行情。
    pub fn row(&self, code: Code) -> Option<MarketRow> {
        let i = self.idx(code)?;
        Some(MarketRow {
            suspended: self.suspended[i],
            limit_buy: self.limit_buy[i],
            limit_sell: self.limit_sell[i],
            volume_cap: self.volume_cap[i],
            deal_price: self.deal_price[i],
        })
    }

    /// 当日复权因子（复权调整与买入入账用）；`None` = 当日无行情。
    pub fn factor(&self, code: Code) -> Option<f64> {
        self.idx(code).map(|i| self.factor[i])
    }

    /// 估值收盘价：未停牌且 close > 0 时返回；停牌 / 无行情 / close 缺失返回 `None`
    /// （调用方沿用最近有效收盘价）。
    pub fn valuation_close(&self, code: Code) -> Option<f64> {
        let i = self.idx(code)?;
        if self.valid_close[i] {
            Some(self.close[i])
        } else {
            None
        }
    }

    /// 构建策略可见的当日可交易性视图（与撮合共用同一份切片）。
    pub fn tradable_info(&self) -> TradableInfo<'a> {
        TradableInfo {
            codes: self.codes,
            suspended: self.suspended,
            limit_buy: self.limit_buy,
            limit_sell: self.limit_sell,
            volume_cap: self.volume_cap,
            deal_price: self.deal_price,
            is_st: self.is_st,
        }
    }
}

/// 单只股票当日行情行（撮合判定所需字段）。
#[derive(Clone, Copy, Debug)]
pub struct MarketRow {
    pub suspended: bool,
    pub limit_buy: bool,
    pub limit_sell: bool,
    pub volume_cap: f64,
    /// deal_price 列价格；无效为 NaN
    pub deal_price: f64,
}

impl MarketRow {
    /// 转为策略可见的可交易性视图。
    pub fn as_tradable(&self, is_st: bool) -> StockTradable {
        StockTradable {
            suspended: self.suspended,
            limit_buy: self.limit_buy,
            limit_sell: self.limit_sell,
            volume_cap: self.volume_cap,
            deal_price: self.deal_price,
            is_st,
        }
    }
}
