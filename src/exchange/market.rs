//! DailyMarketStore（架构 §4.7）：行情注入 Exchange 时预计算 `limit_buy` / `limit_sell`
//! 与双侧（买 / 卖）撮合基价，并提供按日 SoA 视图与 `TradableInfo` 构建
//! （Exchange 撮合与 Strategy 决策共用同一份当日数据，口径一致，架构 D4）。
//!
//! wap 模式（`deal_price = vwapN / twapN`，决策 D14）：wap 数据与 stock_bar 按
//! (date, code) 归并联接；策略可见价 `deal_price` = `pre_close`（决策时点合法已知，
//! 避免未来函数），撮合基价按方向取 `wap_N_*_buy` / `wap_N_*_sell`，量上限按方向取
//! `wap_N_buy_volume` / `wap_N_sell_volume`；涨跌停按方向基价对 pre_close 判定
//! （方向价已剔除触板 tick，判定只额外拦"接近板但从未触板"的委托）。
//! 普通 open/close/vwap 模式双侧同值（同一列价、同一全日量），行为不变。

use crate::data::stock_bar::StockBarStore;
use crate::data::wap::WapStore;
use crate::exchange::rules;
use crate::types::{Code, DayIdx, DealPrice, StockTradable, TradableInfo, WapKind};

/// 注入行情后的按日市场存储：原始行情 + 预计算列，按 (DayIdx, Code) 排序。
///
/// 原始行情 `bar` 为 `Arc` 共享（决策 D15）：同一份数据跨多次回测复用，
/// 本存储内依赖撮合参数的派生列（limit / 量上限 / 方向基价）每次装配重建。
pub struct DailyMarketStore {
    bar: std::sync::Arc<StockBarStore>,
    /// 策略可见价（委托定价与金额换算）：普通模式 = deal_price 列；wap 模式 = pre_close；
    /// 缺失 / NaN / ≤ 0 时为 NaN，当日该股不可交易
    deal_price: Vec<f64>,
    limit_buy: Vec<bool>,
    limit_sell: Vec<bool>,
    /// 当日买入侧成交量上限：volume × volume_threshold（wap 模式 = buy_volume × threshold）；
    /// None -> +∞；无量 / 缺失 -> 0
    volume_cap: Vec<f64>,
    /// 停牌：paused = 1 或 close 缺失
    suspended: Vec<bool>,
    /// 估值可用：未停牌且 close > 0
    valid_close: Vec<bool>,
    /// wap 模式专属列（普通模式 None：撮合基价回落 deal_price、卖侧量上限回落 volume_cap）
    wap_side: Option<WapSide>,
}

/// wap 模式专属的按行预计算列（与 stock_bar 行对齐）。
struct WapSide {
    /// 撮合买侧基价（`wap_N_*_buy`，无效为 NaN）
    exec_buy: Vec<f64>,
    /// 撮合卖侧基价（`wap_N_*_sell`，无效为 NaN）
    exec_sell: Vec<f64>,
    /// 卖出侧成交量上限（`sell_volume × volume_threshold`）
    sell_volume_cap: Vec<f64>,
}

/// 价格归一：缺失 / NaN / ≤ 0 -> NaN（不可成交标记）。
fn norm_price(p: f64) -> f64 {
    if p.is_nan() || p <= 0.0 {
        f64::NAN
    } else {
        p
    }
}

/// 方向量上限：量缺失 / ≤ 0 -> 0；threshold = None -> ∞；否则量 × threshold。
fn direction_cap(volume: f64, threshold: Option<f64>) -> f64 {
    if volume.is_nan() || volume <= 0.0 {
        0.0
    } else {
        match threshold {
            Some(t) => volume * t,
            None => f64::INFINITY,
        }
    }
}

impl DailyMarketStore {
    /// 注入行情：按 deal_price 预计算可交易性相关列。
    ///
    /// `wap` 在 `deal_price` 为 `Wap` 时必须提供且时段一致（`inject_market` 已校验）；
    /// 普通模式应传 `None`（提供则由调用方告警忽略）。
    pub fn build(
        bar: std::sync::Arc<StockBarStore>,
        wap: Option<&WapStore>,
        deal_price_col: DealPrice,
        volume_threshold: Option<f64>,
        limit_threshold: Option<f64>,
    ) -> Self {
        let n = bar.codes.len();
        let price_col: Option<&[f64]> = match deal_price_col {
            DealPrice::Open => Some(&bar.open),
            DealPrice::Close => Some(&bar.close),
            DealPrice::Vwap => Some(&bar.vwap),
            DealPrice::Wap { .. } => None,
        };
        let wap_sel: Option<(WapKind, u8)> = match deal_price_col {
            DealPrice::Wap { kind, window } => Some((kind, window)),
            _ => None,
        };

        let mut deal_price = Vec::with_capacity(n);
        let mut limit_buy = Vec::with_capacity(n);
        let mut limit_sell = Vec::with_capacity(n);
        let mut volume_cap = Vec::with_capacity(n);
        let mut suspended = Vec::with_capacity(n);
        let mut valid_close = Vec::with_capacity(n);
        // wap 模式专属列（普通模式保持空，day_view 回落）
        let mut exec_buy = Vec::new();
        let mut exec_sell = Vec::new();
        let mut sell_volume_cap = Vec::new();
        if wap_sel.is_some() {
            exec_buy.reserve(n);
            exec_sell.reserve(n);
            sell_volume_cap.reserve(n);
        }

        let mut missing_limit_warn = 0usize;
        let threshold_ratio = limit_threshold.map(|t| t / 0.1);

        // 归并联接游标：stock_bar 行序 (DayIdx, Code) 与 wap 行序 (date, code) 同序，
        // 单调推进即可（两侧键均无重复）
        let mut w = 0usize;

        for i in 0..n {
            let paused = bar.paused[i];
            let close = bar.close[i];
            let susp = paused || close.is_nan();
            suspended.push(susp);
            valid_close.push(!susp && close > 0.0);

            let pre = bar.pre_close[i];
            let hl = bar.high_limit[i];
            let ll = bar.low_limit[i];

            match wap_sel {
                Some((kind, _)) => {
                    let wstore = wap.expect("inject_market 已校验 wap 模式必提供 wap 数据");
                    // 联接：定位 (date, code) 匹配的 wap 行
                    let date = bar.calendar.date(bar.days[i]);
                    let code = bar.codes[i];
                    while w < wstore.dates.len()
                        && (wstore.dates[w], wstore.codes[w]) < (date, code)
                    {
                        w += 1;
                    }
                    let joined = if w < wstore.dates.len()
                        && (wstore.dates[w], wstore.codes[w]) == (date, code)
                    {
                        Some(w)
                    } else {
                        None
                    };

                    // 策略可见价 = pre_close（决策时点已知；无效 -> NaN）
                    deal_price.push(norm_price(pre));

                    // 方向撮合基价：wap 缺行或该方向无可成交 tick（价 ≤ 0 / 缺失）-> NaN
                    let (ebuy, esell) = match joined.map(|j| match kind {
                        WapKind::Vwap => (wstore.vwap_buy[j], wstore.vwap_sell[j]),
                        WapKind::Twap => (wstore.twap_buy[j], wstore.twap_sell[j]),
                    }) {
                        Some((pb, ps)) => (norm_price(pb), norm_price(ps)),
                        None => (f64::NAN, f64::NAN),
                    };
                    exec_buy.push(ebuy);
                    exec_sell.push(esell);

                    // 双侧量上限（wap 缺行 -> 0）
                    let (bvol, svol) = match joined {
                        Some(j) => (wstore.buy_volume[j], wstore.sell_volume[j]),
                        None => (f64::NAN, f64::NAN),
                    };
                    volume_cap.push(direction_cap(bvol, volume_threshold));
                    sell_volume_cap.push(direction_cap(svol, volume_threshold));

                    // 涨跌停预计算（判定顺序：先停牌后涨跌停；方向价无效侧置 false，
                    // 由撮合时的基价 / 量上限检查拦截）
                    let (lb, ls) = match threshold_ratio {
                        None => (false, false),
                        Some(r) => {
                            if pre.is_nan() || pre <= 0.0 {
                                (false, false)
                            } else if hl.is_nan() || ll.is_nan() || hl <= 0.0 || ll <= 0.0 {
                                missing_limit_warn += 1;
                                (false, false)
                            } else {
                                let lb = if ebuy.is_nan() {
                                    false
                                } else {
                                    rules::limit_flags(pre, hl, ll, ebuy, r).0
                                };
                                let ls = if esell.is_nan() {
                                    false
                                } else {
                                    rules::limit_flags(pre, hl, ll, esell, r).1
                                };
                                (lb, ls)
                            }
                        }
                    };
                    limit_buy.push(lb);
                    limit_sell.push(ls);
                }
                None => {
                    // 普通模式（原口径，行为不变）
                    let p = price_col.expect("非 wap 模式必有 stock_bar 价格列");
                    deal_price.push(norm_price(p[i]));

                    let cap = direction_cap(bar.volume[i], volume_threshold);
                    volume_cap.push(cap);

                    let (lb, ls) = match threshold_ratio {
                        None => (false, false), // None: 不做涨跌停限制（构建时已 warning）
                        Some(r) => {
                            if pre.is_nan() || pre <= 0.0 {
                                // pre_close 缺失（如上市首日）：不做涨跌停判定，不告警
                                (false, false)
                            } else if hl.is_nan() || ll.is_nan() || hl <= 0.0 || ll <= 0.0 {
                                // high_limit / low_limit 自身缺失：置 false 并 warning（聚合计数）
                                missing_limit_warn += 1;
                                (false, false)
                            } else if p[i].is_nan() || p[i] <= 0.0 {
                                // deal_price 列无效：不可交易，limit 标记无意义
                                (false, false)
                            } else {
                                rules::limit_flags(pre, hl, ll, p[i], r)
                            }
                        }
                    };
                    limit_buy.push(lb);
                    limit_sell.push(ls);
                }
            }
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
            wap_side: if wap_sel.is_some() {
                Some(WapSide {
                    exec_buy,
                    exec_sell,
                    sell_volume_cap,
                })
            } else {
                None
            },
        }
    }

    /// 行情全部股票 code 集（信号"instrument 无行情"校验用）。
    pub fn code_set(&self) -> &std::collections::HashSet<Code> {
        &self.bar.code_set
    }

    /// 取当日 SoA 视图。
    pub fn day_view(&self, day: DayIdx) -> DayView<'_> {
        let range = self.bar.day_range(day);
        // 普通模式回落：撮合基价 = deal_price、卖侧量上限 = 买侧量上限（双侧同值）
        let (exec_buy, exec_sell, sell_volume_cap) = match &self.wap_side {
            Some(ws) => (
                &ws.exec_buy[range.clone()],
                &ws.exec_sell[range.clone()],
                &ws.sell_volume_cap[range.clone()],
            ),
            None => (
                &self.deal_price[range.clone()],
                &self.deal_price[range.clone()],
                &self.volume_cap[range.clone()],
            ),
        };
        DayView {
            codes: &self.bar.codes[range.clone()],
            deal_price: &self.deal_price[range.clone()],
            limit_buy: &self.limit_buy[range.clone()],
            limit_sell: &self.limit_sell[range.clone()],
            volume_cap: &self.volume_cap[range.clone()],
            sell_volume_cap,
            suspended: &self.suspended[range.clone()],
            valid_close: &self.valid_close[range.clone()],
            close: &self.bar.close[range.clone()],
            factor: &self.bar.factor[range.clone()],
            is_st: &self.bar.is_st[range],
            exec_buy,
            exec_sell,
        }
    }
}

/// 单日市场视图：撮合与账户估值共用。
#[derive(Clone, Copy)]
pub struct DayView<'a> {
    pub codes: &'a [Code],
    /// 策略可见价（普通模式 = deal_price 列；wap 模式 = pre_close）
    pub deal_price: &'a [f64],
    pub limit_buy: &'a [bool],
    pub limit_sell: &'a [bool],
    /// 买入侧成交量上限
    pub volume_cap: &'a [f64],
    /// 卖出侧成交量上限（普通模式与 `volume_cap` 同切片）
    pub sell_volume_cap: &'a [f64],
    pub suspended: &'a [bool],
    pub valid_close: &'a [bool],
    pub close: &'a [f64],
    pub factor: &'a [f64],
    pub is_st: &'a [bool],
    /// 撮合买侧基价（普通模式与 `deal_price` 同切片）
    pub exec_buy: &'a [f64],
    /// 撮合卖侧基价（普通模式与 `deal_price` 同切片）
    pub exec_sell: &'a [f64],
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
            sell_volume_cap: self.sell_volume_cap[i],
            deal_price: self.deal_price[i],
            exec_buy: self.exec_buy[i],
            exec_sell: self.exec_sell[i],
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
            sell_volume_cap: self.sell_volume_cap,
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
    /// 买入侧成交量上限
    pub volume_cap: f64,
    /// 卖出侧成交量上限（普通模式与 `volume_cap` 同值）
    pub sell_volume_cap: f64,
    /// 策略可见价（无效为 NaN）
    pub deal_price: f64,
    /// 撮合买侧基价（无效为 NaN）
    pub exec_buy: f64,
    /// 撮合卖侧基价（无效为 NaN）
    pub exec_sell: f64,
}

impl MarketRow {
    /// 转为策略可见的可交易性视图。
    pub fn as_tradable(&self, is_st: bool) -> StockTradable {
        StockTradable {
            suspended: self.suspended,
            limit_buy: self.limit_buy,
            limit_sell: self.limit_sell,
            volume_cap: self.volume_cap,
            sell_volume_cap: self.sell_volume_cap,
            deal_price: self.deal_price,
            is_st,
        }
    }
}
