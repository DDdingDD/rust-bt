//! Exchange（模拟交易所，架构 §4.7）：参数校验、行情注入、deal_order 撮合。

pub mod market;
pub mod rules;

pub use market::{DailyMarketStore, DayView};

use crate::account::Account;
use crate::data::stock_bar::StockBarStore;
use crate::data::wap::WapStore;
use crate::error::{BtError, Result};
use crate::order::Order;
use crate::types::{DayIdx, DealPrice};

/// 交易所费用与约束参数。
#[derive(Clone, Copy, Debug)]
pub struct ExchangeConfig {
    pub deal_price: DealPrice,
    pub open_cost: f64,
    pub close_cost: f64,
    pub min_cost: f64,
    pub fixed_slippage: f64,
    pub min_slippage_ratio: f64,
    pub volume_threshold: Option<f64>,
    pub limit_threshold: Option<f64>,
}

/// 模拟交易所。行情由 `Backtest::new` 装配时注入（`Exchange::new` 只接收费用与约束参数）。
pub struct Exchange {
    config: ExchangeConfig,
    market: Option<DailyMarketStore>,
}

impl Exchange {
    /// 构造期校验：deal_price 合法；limit_threshold ∈ (0, 0.1]，越界 Err；
    /// None -> 不做涨跌停限制并 warning（涨跌停的股票也能被买卖）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        deal_price: &str,
        open_cost: f64,
        close_cost: f64,
        min_cost: f64,
        fixed_slippage: f64,
        min_slippage_ratio: f64,
        volume_threshold: Option<f64>,
        limit_threshold: Option<f64>,
    ) -> Result<Self> {
        Self::with_deal_price(
            DealPrice::parse(deal_price)?,
            open_cost,
            close_cost,
            min_cost,
            fixed_slippage,
            min_slippage_ratio,
            volume_threshold,
            limit_threshold,
        )
    }

    /// 类型化构造（嵌入 API `BtParams` 路径）：`deal_price` 为已解析枚举，
    /// 校验口径与 [`Exchange::new`] 一致。
    #[allow(clippy::too_many_arguments)]
    pub fn with_deal_price(
        deal_price: DealPrice,
        open_cost: f64,
        close_cost: f64,
        min_cost: f64,
        fixed_slippage: f64,
        min_slippage_ratio: f64,
        volume_threshold: Option<f64>,
        limit_threshold: Option<f64>,
    ) -> Result<Self> {
        for (name, v) in [
            ("open_cost", open_cost),
            ("close_cost", close_cost),
            ("min_cost", min_cost),
            ("fixed_slippage", fixed_slippage),
            ("min_slippage_ratio", min_slippage_ratio),
        ] {
            if !(v >= 0.0) || v.is_nan() {
                return Err(BtError::InvalidParam(format!("{name} 须为非负数，收到: {v}")));
            }
        }
        if let Some(t) = volume_threshold {
            if !(t >= 0.0) || t.is_nan() {
                return Err(BtError::InvalidParam(format!(
                    "volume_threshold 须为非负数，收到: {t}"
                )));
            }
        }
        match limit_threshold {
            Some(t) if !(t > 0.0 && t <= 0.1) || t.is_nan() => {
                return Err(BtError::InvalidParam(format!(
                    "limit_threshold 须在 (0, 0.1] 区间，收到: {t}"
                )));
            }
            None => {
                log::warn!("limit_threshold = None：不做涨跌停限制，涨跌停的股票也能被买卖");
            }
            _ => {}
        }
        Ok(Self {
            config: ExchangeConfig {
                deal_price,
                open_cost,
                close_cost,
                min_cost,
                fixed_slippage,
                min_slippage_ratio,
                volume_threshold,
                limit_threshold,
            },
            market: None,
        })
    }

    /// 注入行情（由 `Backtest::new` 调用）：按 deal_price 预计算 limit 列。
    ///
    /// 装配期校验 wap 数据与 deal_price 匹配：wap 模式（vwapN/twapN）必须提供 wap
    /// 数据且时段一致；普通模式提供了 wap 数据则告警忽略。
    pub(crate) fn inject_market(
        &mut self,
        bar: StockBarStore,
        wap: Option<WapStore>,
    ) -> Result<()> {
        match self.config.deal_price {
            DealPrice::Wap { window, .. } => {
                let wap = wap.ok_or_else(|| {
                    BtError::InvalidParam(format!(
                        "deal_price={} 需要 wap 时段数据，请经 BTData::load_wap / BtParams.wap 提供",
                        self.config.deal_price
                    ))
                })?;
                if wap.window != window {
                    return Err(BtError::InvalidParam(format!(
                        "wap 数据时段（wap_{}）与 deal_price={} 不一致，请按对应时段 load_wap",
                        wap.window, self.config.deal_price
                    )));
                }
                self.market = Some(DailyMarketStore::build(
                    bar,
                    Some(&wap),
                    self.config.deal_price,
                    self.config.volume_threshold,
                    self.config.limit_threshold,
                ));
            }
            _ => {
                if let Some(w) = &wap {
                    log::warn!(
                        "已加载 wap 时段数据（wap_{}）但 deal_price={} 未使用 vwapN/twapN，忽略",
                        w.window,
                        self.config.deal_price
                    );
                }
                self.market = Some(DailyMarketStore::build(
                    bar,
                    None,
                    self.config.deal_price,
                    self.config.volume_threshold,
                    self.config.limit_threshold,
                ));
            }
        }
        Ok(())
    }

    /// 行情全部股票 code 集。
    pub(crate) fn code_set(&self) -> &std::collections::HashSet<crate::types::Code> {
        self.market
            .as_ref()
            .expect("行情须由 Backtest::new 注入")
            .code_set()
    }

    /// 取当日市场视图。
    pub(crate) fn day_view(&self, day: DayIdx) -> DayView<'_> {
        self.market
            .as_ref()
            .expect("行情须由 Backtest::new 注入")
            .day_view(day)
    }

    /// 单订单撮合（规范"撮合通用规则"流水线）：
    /// 可交易性 -> 裁剪 -> 滑点 -> 资金反解 -> 整手 -> 费用 -> 回填 -> account.on_deal。
    ///
    /// wap 模式（vwapN/twapN）：撮合基价与量上限按方向取 wap 列（买 `_buy`、卖 `_sell`），
    /// 委托价（策略按 pre_close 填写）仅作防御校验，不参与成交价。
    pub fn deal_order(&self, order: &mut Order, account: &mut Account, day: DayIdx) {
        order.deal_volume = 0.0;
        order.deal_price = 0.0;
        order.deal_cost = 0.0;

        let view = self.day_view(day);
        let cfg = &self.config;

        // 1. 当日无行情（退市）-> 不成交
        let Some(row) = view.row(order.stock) else {
            return;
        };
        // 2. 停牌检查（先行！）
        if row.suspended {
            return;
        }
        let is_buy = order.volume > 0.0;
        if order.volume == 0.0 {
            return;
        }
        // 3. 涨跌停检查
        if is_buy && row.limit_buy {
            return;
        }
        if !is_buy && row.limit_sell {
            return;
        }
        // 委托价格防御：内置策略取当日可见价（普通模式 = deal_price 列，wap 模式 = pre_close）；
        // 非法价格不成交
        if order.price.is_nan() || order.price <= 0.0 {
            return;
        }
        // 方向撮合基价（wap 模式 = wap_N_*_buy / _sell；普通模式 = deal_price 列）
        // 与方向量上限；基价无效（该方向无可成交 tick / 列无效）-> 不可交易
        let exec = if is_buy { row.exec_buy } else { row.exec_sell };
        if exec.is_nan() {
            return;
        }
        // 4. 成交量裁剪（方向量上限；当日无量 -> 0）
        let cap = if is_buy {
            row.volume_cap
        } else {
            row.sell_volume_cap
        };
        let mut qty = order.volume.abs().min(cap);
        if qty <= 0.0 {
            return;
        }
        // 5. 卖出截断至持仓量（warning）
        if !is_buy {
            let held = account
                .positions()
                .get(&order.stock)
                .map(|e| e.volume)
                .unwrap_or(0.0);
            if qty > held + 1e-9 {
                log::warn!(
                    "卖出委托量 {qty} 超过持仓量 {held}（code={}），截断至持仓量",
                    order.stock
                );
                qty = held;
            }
            if qty <= 0.0 {
                return;
            }
        }
        // 6. 滑点：在撮合基价上叠加，按方向调整，不 clamp。
        // 普通模式基价 = 委托价（策略取 deal_price 列）；wap 模式基价 = 方向行情价
        let base_price = match cfg.deal_price {
            DealPrice::Wap { .. } => exec,
            _ => order.price,
        };
        let ratio =
            rules::slippage_ratio(cfg.fixed_slippage, cfg.min_slippage_ratio, base_price);
        let deal_price = rules::apply_slippage(base_price, ratio, is_buy);
        // 7. 买单资金约束反解（用滑点后成交价）
        if is_buy {
            let affordable =
                rules::max_buyable_shares(account.cash(), deal_price, cfg.open_cost, cfg.min_cost);
            qty = qty.min(affordable);
            // 8. 整手取整（买入；卖出不取整，允许零股）
            qty = rules::round_buy_lot(qty, order.stock);
            if qty <= 0.0 {
                return;
            }
        }
        // 9. 费用（成交量为 0 已在前面 return）
        let trade_val = qty * deal_price;
        let cost_ratio = if is_buy { cfg.open_cost } else { cfg.close_cost };
        let cost = rules::trade_cost(trade_val, cost_ratio, cfg.min_cost);
        // 10. 回填 + 落账
        order.deal_volume = if is_buy { qty } else { -qty };
        order.deal_price = deal_price;
        order.deal_cost = cost;
        account.on_deal(order, &view);
    }
}
