//! Account（账户，架构 §4.5）：记账、双资产口径源数据、日终估值、逐日记录。
//!
//! 账户是 Exchange 撮合结果的落账方，也是 Report 指标的记录者。

use crate::exchange::market::DayView;
use crate::order::Order;
use crate::position::{adjust_entry_factor, PositionEntry, Positions};
use crate::types::{Code, DayIdx};

/// 逐日账户记录（PortfolioMetrics 源数据）。
#[derive(Clone, Copy, Debug)]
pub struct DailyRecord {
    pub day: DayIdx,
    /// 当日总资产（含成本口径）
    pub account: f64,
    /// 当日持仓市值
    pub value: f64,
    /// 当日现金（允许为负：卖出费用超成交金额情形）
    pub cash: f64,
    /// 当日双边成交金额 Σ(deal_price × deal_volume)，含滑点口径
    pub turnover_amount: f64,
    /// 当日交易费用
    pub cost: f64,
}

/// 逐日持仓快照行（导出 hist_position 用；weight 导出时按当日总资产计算）。
#[derive(Clone, Copy, Debug)]
pub struct HistPositionRow {
    pub day: DayIdx,
    pub code: Code,
    pub volume: f64,
    pub cost_price: f64,
    pub price: f64,
    pub count_day: u32,
}

/// 回测交易账户。
pub struct Account {
    cash: f64,
    positions: Positions,
    daily: Vec<DailyRecord>,
    hist_positions: Vec<HistPositionRow>,
    /// 当日累计双边成交金额（end_of_day 落账后清零）
    day_turnover: f64,
    /// 当日累计交易费用（end_of_day 落账后清零）
    day_cost: f64,
}

impl Account {
    /// 期初全现金。`cash` 须为正有限值。
    pub fn new(cash: f64) -> Self {
        assert!(
            cash.is_finite() && cash > 0.0,
            "Account::new 期初现金须为正有限值，收到: {cash}"
        );
        Self {
            cash,
            positions: Positions::new(),
            daily: Vec::new(),
            hist_positions: Vec::new(),
            day_turnover: 0.0,
            day_cost: 0.0,
        }
    }

    pub fn cash(&self) -> f64 {
        self.cash
    }

    pub fn positions(&self) -> &Positions {
        &self.positions
    }

    /// 取出逐日账户记录（Backtest 装配 BTResult 时调用）。
    pub(crate) fn take_daily(&mut self) -> Vec<DailyRecord> {
        std::mem::take(&mut self.daily)
    }

    /// 取出逐日持仓快照（Backtest 装配 BTResult 时调用）。
    pub(crate) fn take_hist_positions(&mut self) -> Vec<HistPositionRow> {
        std::mem::take(&mut self.hist_positions)
    }

    /// 撮合前复权调整（主循环第 2 步）：对当日 `factor ≠ last_factor` 的持仓
    /// 调整 `volume` / `cost_price` / `last_factor`。当日新买入尚未入账，天然不参与。
    pub fn adjust_factor(&mut self, view: &DayView) {
        for (code, entry) in self.positions.iter_mut() {
            if let Some(factor) = view.factor(*code) {
                adjust_entry_factor(entry, factor);
            }
        }
    }

    /// 成交落账（主循环第 4 步）：现金增减（含费用）、持仓更新、累计当日成交口径。
    ///
    /// 约定：`order.deal_volume` 带符号（买正卖负），`deal_price` 为含滑点成交价。
    pub fn on_deal(&mut self, order: &Order, view: &DayView) {
        if order.deal_volume == 0.0 {
            return;
        }
        let signed_val = order.deal_volume * order.deal_price;
        // 买入：现金减少 成交金额 + 费用；卖出（deal_volume < 0）：现金增加 |成交金额| − 费用
        self.cash -= signed_val + order.deal_cost;
        self.day_turnover += signed_val.abs();
        self.day_cost += order.deal_cost;

        let factor = view.factor(order.stock).unwrap_or(1.0);
        if order.deal_volume > 0.0 {
            match self.positions.get_mut(&order.stock) {
                Some(e) => {
                    // 加仓：加权平均成本；count_day 不重置
                    let total = e.volume + order.deal_volume;
                    e.cost_price =
                        (e.volume * e.cost_price + order.deal_volume * order.deal_price) / total;
                    e.volume = total;
                    e.last_factor = factor;
                }
                None => {
                    // 新买入：cost_price = 实际成交价（含滑点，不含费用）；
                    // count_day 置 0，end_of_day 统一 +1 -> 买入成交日记 1
                    self.positions.insert(
                        order.stock,
                        PositionEntry {
                            volume: order.deal_volume,
                            cost_price: order.deal_price,
                            price: order.deal_price,
                            last_factor: factor,
                            count_day: 0,
                        },
                    );
                }
            }
        } else if let Some(e) = self.positions.get_mut(&order.stock) {
            e.volume += order.deal_volume; // deal_volume 为负
            if e.volume <= 1e-9 {
                self.positions.remove(&order.stock);
            }
            // 部分卖出：cost_price / count_day 不变
        } else {
            log::warn!(
                "on_deal: 卖出委托 {}{} 但持仓中无该股票，忽略",
                order.deal_volume,
                order.stock
            );
        }
    }

    /// 日终估值与记账（主循环第 5 步）：
    /// 有有效行情的持仓更新 `price`（停牌 / 退市沿用最近有效收盘价）；
    /// `count_day +1`（当日新买入在 on_deal 记 0，此处 +1 即记 1）；
    /// 记录逐日账户与持仓快照。
    pub fn end_of_day(&mut self, view: &DayView, day: DayIdx) {
        let mut value = 0.0;
        for (code, e) in self.positions.iter_mut() {
            if let Some(close) = view.valuation_close(*code) {
                e.price = close;
            }
            e.count_day += 1;
            value += e.market_value();
        }
        let account = self.cash + value;
        self.daily.push(DailyRecord {
            day,
            account,
            value,
            cash: self.cash,
            turnover_amount: self.day_turnover,
            cost: self.day_cost,
        });
        for (code, e) in &self.positions {
            self.hist_positions.push(HistPositionRow {
                day,
                code: *code,
                volume: e.volume,
                cost_price: e.cost_price,
                price: e.price,
                count_day: e.count_day,
            });
        }
        self.day_turnover = 0.0;
        self.day_cost = 0.0;
    }
}
